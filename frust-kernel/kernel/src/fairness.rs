//! Tenant fairness at the doors.
//!
//! The kernel has exactly two doors — the broker (verbs) and the worker
//! (jobs) — which is what makes fairness enforceable at all. This module is
//! the accounting and the verdict; the doors call it.
//!
//! **Throttling is loud.** Over budget returns `E_TENANT_THROTTLED` with the
//! wait hint, never a silent slow: a tenant that is being shaped can see that
//! it is being shaped (house style, applied to quotas).
//!
//! Policy lives in `_tenant_policy` — kernel-owned DDL like `app_user`. Absent
//! policy = unlimited, so fairness is opt-in per deployment and the default
//! posture is unchanged.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::contract::BrokerError;
use crate::telemetry;

/// Kernel-owned policy table. Its DDL lives in `meta.rs` (static kernel DDL,
/// like identity) and its rows are loaded by `boot.rs`; this module holds the
/// accounting and the verdict, never query text.
pub const POLICY_TABLE: &str = "_tenant_policy";

/// A tenant's shape: sustained rate + burst depth, per door.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    /// Verb executions per second (sustained).
    pub verbs_per_sec: f64,
    /// Burst depth in verbs — how far ahead a quiet tenant may spend.
    pub verb_burst: f64,
    /// Max jobs one tenant may run per worker scheduling round.
    pub jobs_per_round: usize,
}

impl Default for Policy {
    fn default() -> Self {
        // unlimited: fairness is opt-in, the default posture is unchanged
        Self { verbs_per_sec: f64::INFINITY, verb_burst: f64::INFINITY, jobs_per_round: usize::MAX }
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
    policy: Policy,
}

impl Bucket {
    fn new(policy: Policy) -> Self {
        Self { tokens: policy.verb_burst, last: Instant::now(), policy }
    }

    /// Refills by elapsed time, then spends one token if available.
    /// Returns `Ok(())` or the seconds a caller should wait.
    fn take(&mut self) -> Result<(), f64> {
        if self.policy.verbs_per_sec.is_infinite() {
            return Ok(());
        }
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64();
        self.last = now;
        self.tokens = (self.tokens + elapsed * self.policy.verbs_per_sec).min(self.policy.verb_burst);
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            Ok(())
        } else {
            Err((1.0 - self.tokens) / self.policy.verbs_per_sec)
        }
    }
}

#[derive(Default)]
struct State {
    buckets: HashMap<String, Bucket>,
    policies: HashMap<String, Policy>,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

/// Installs (or replaces) a tenant's policy. Loaded from `_tenant_policy` at
/// boot; also the seam tests and operators use.
pub fn set_policy(tenant: &str, policy: Policy) {
    let mut s = state().lock().unwrap();
    s.policies.insert(tenant.to_string(), policy);
    s.buckets.insert(tenant.to_string(), Bucket::new(policy));
    telemetry::gauge("frust_tenant_verbs_per_sec", &[("tenant", tenant)], policy.verbs_per_sec);
    telemetry::gauge(
        "frust_tenant_jobs_per_round",
        &[("tenant", tenant)],
        if policy.jobs_per_round == usize::MAX { -1.0 } else { policy.jobs_per_round as f64 },
    );
}

pub fn policy_for(tenant: &str) -> Policy {
    state().lock().unwrap().policies.get(tenant).copied().unwrap_or_default()
}

pub fn clear() {
    let mut s = state().lock().unwrap();
    s.buckets.clear();
    s.policies.clear();
}

/// The broker door's admission check. Loud refusal carries the wait hint.
pub fn admit_verb(tenant: &str) -> Result<(), BrokerError> {
    let verdict = {
        let mut s = state().lock().unwrap();
        let policy = s.policies.get(tenant).copied().unwrap_or_default();
        s.buckets.entry(tenant.to_string()).or_insert_with(|| Bucket::new(policy)).take()
    };
    match verdict {
        Ok(()) => {
            telemetry::inc("frust_tenant_verbs_admitted_total", &[("tenant", tenant)], 1);
            Ok(())
        }
        Err(retry_after) => {
            telemetry::inc("frust_tenant_verbs_throttled_total", &[("tenant", tenant)], 1);
            telemetry::emit(
                telemetry::Level::Info,
                "tenant_throttled",
                &[("retry_after_ms", serde_json::json!((retry_after * 1000.0).ceil() as u64))],
            );
            Err(BrokerError::TenantThrottled { retry_after_ms: (retry_after * 1000.0).ceil() as u64 })
        }
    }
}

// ── worker door: round-robin across tenants with queued work ────────────────

/// Advances once per round so successive rounds start at a different tenant.
/// Without this, a caller that takes only the FIRST job of each round (which
/// is exactly what `claim_next` does) would always serve the same tenant —
/// a "fair order" nobody ever reads past position zero.
static ROUND: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Orders a claim round so no tenant's backlog starves another's work.
///
/// Input is the queue head (job id, tenant) in enqueue order; output is the
/// same jobs re-ordered round-robin by tenant, each tenant capped at its
/// `jobs_per_round`, with the starting tenant rotating per round. A tenant
/// with 500 queued and a tenant with 5 queued interleave, so the second
/// tenant's jobs never wait behind the first's entire backlog.
pub fn fair_round(queued: &[(String, String)]) -> Vec<String> {
    let mut by_tenant: Vec<(String, Vec<String>)> = Vec::new();
    for (job, tenant) in queued {
        match by_tenant.iter_mut().find(|(t, _)| t == tenant) {
            Some((_, jobs)) => jobs.push(job.clone()),
            None => by_tenant.push((tenant.clone(), vec![job.clone()])),
        }
    }
    if by_tenant.len() > 1 {
        let start = ROUND.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % by_tenant.len();
        by_tenant.rotate_left(start);
    }
    let caps: Vec<usize> = by_tenant.iter().map(|(t, _)| policy_for(t).jobs_per_round).collect();
    let mut taken = vec![0usize; by_tenant.len()];
    let mut out = Vec::with_capacity(queued.len());
    let mut progressed = true;
    while progressed {
        progressed = false;
        for (i, (_, jobs)) in by_tenant.iter().enumerate() {
            if taken[i] >= caps[i] {
                continue; // this tenant has had its share of the round
            }
            if let Some(job) = jobs.get(taken[i]) {
                out.push(job.clone());
                taken[i] += 1;
                progressed = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_policy_is_unlimited() {
        for _ in 0..1000 {
            admit_verb("no_policy_tenant").expect("unlimited by default");
        }
    }

    #[test]
    fn bucket_throttles_loudly_then_refills() {
        set_policy("t_burst", Policy { verbs_per_sec: 100.0, verb_burst: 3.0, jobs_per_round: 1 });
        for _ in 0..3 {
            admit_verb("t_burst").expect("burst depth spends freely");
        }
        let err = admit_verb("t_burst").unwrap_err();
        match err {
            BrokerError::TenantThrottled { retry_after_ms } => {
                assert!(retry_after_ms <= 20, "wait hint is actionable: {retry_after_ms} ms");
            }
            other => panic!("expected a typed throttle, got {other:?}"),
        }
        // refill: 100/s means ~10 ms per token
        std::thread::sleep(std::time::Duration::from_millis(40));
        admit_verb("t_burst").expect("bucket refills with time");
    }

    #[test]
    fn one_tenants_budget_does_not_touch_another() {
        set_policy("t_small", Policy { verbs_per_sec: 1.0, verb_burst: 1.0, jobs_per_round: 1 });
        admit_verb("t_small").unwrap();
        assert!(admit_verb("t_small").is_err(), "t_small is spent");
        for _ in 0..50 {
            admit_verb("t_unshaped_neighbour").expect("a throttled neighbour cannot throttle me");
        }
    }

    /// The 500-vs-5 shape: the trickle's jobs must not sit behind the
    /// flood's five hundred. (Distinct tenant names per test: policy state
    /// is process-global, and cargo runs these in parallel.)
    #[test]
    fn fair_round_interleaves_a_flood_with_a_trickle() {
        let mut queued: Vec<(String, String)> =
            (0..500).map(|i| (format!("f{i}"), "FLOOD".into())).collect();
        queued.extend((0..5).map(|i| (format!("t{i}"), "TRICKLE".to_string())));

        let order = fair_round(&queued);
        assert_eq!(order.len(), 505, "every job still runs");

        // the trickle's LAST job lands in the first handful of slots
        let last_t = order.iter().position(|j| j == "t4").expect("t4 scheduled");
        assert!(last_t <= 10, "trickle's last job runs at slot {last_t}, not behind the flood");
        // and the flood is not starved either: it holds the majority of slots
        assert!(order.iter().filter(|j| j.starts_with('f')).count() == 500);
    }

    /// Per-round caps bound how much of a round one tenant may take.
    #[test]
    fn jobs_per_round_caps_a_floods_share() {
        set_policy("CAPPED", Policy { jobs_per_round: 2, ..Policy::default() });
        let mut queued: Vec<(String, String)> =
            (0..10).map(|i| (format!("c{i}"), "CAPPED".into())).collect();
        queued.extend((0..3).map(|i| (format!("u{i}"), "UNCAPPED".to_string())));
        let order = fair_round(&queued);
        assert_eq!(
            order.iter().filter(|j| j.starts_with('c')).count(),
            2,
            "the capped tenant takes only its per-round share"
        );
        assert_eq!(order.iter().filter(|j| j.starts_with('u')).count(), 3, "the uncapped tenant drains");
    }
}
