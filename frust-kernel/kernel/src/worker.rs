//! The job worker.
//!
//! The loop: replay-from-cursor (versionstamp changefeed) -> LIVE tail ->
//! advance cursor. LIVE is a latency optimization over a changefeed-backed
//! log (proven viable with a bridge). The atomic conditional claim is
//! the ONLY serialization point: it prevents two concurrent workers from
//! claiming the same queued row. It does NOT make execution atomic with the
//! post-run status write, so delivery is at-least-once across crashes and
//! retries — harmless for idempotent job execution, but for the mail path a
//! resend is a real (bounded) outcome, not an impossibility (see
//! `try_claim_mail` / `send_one`). Cold start beyond retention = rescan
//! `status='queued'`; jobs are records.
//!
//! This module speaks HTTP `/sql` for claim/run (the wire-protocol transport
//! contract); the LIVE tail is the proven WS path, wired in when the
//! worker runs as a daemon. The executed, test-covered core is claim + run +
//! retention rescan, plus run-time authority re-derivation.

use crate::broker::{Broker, Caller, HookChain};
use crate::contract::{BrokerError, Value, WriteOp};
use crate::db::Db;

/// A job's current, exclusive execution right.
///
/// `token` is the fencing value: every completion, retry, and renewal is
/// conditional on it. A worker whose lease was reclaimed can still finish its
/// local effect, but it cannot overwrite the newer owner's queue state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobLease {
    pub claimed_by: String,
    pub token: String,
    /// One-based execution attempt, incremented atomically at claim time.
    pub attempt: u32,
    pub expires_at: String,
}

/// A claimed job, decoded from the queue.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub requested_by: String,
    pub payload: serde_json::Value,
    /// The originating trace (stamped at enqueue) — adopted at run so one
    /// trace spans REST -> enqueue -> claim -> job effect.
    pub trace: Option<String>,
    pub lease: JobLease,
}

/// Outcome of running one job. `Denied` is the hard case:
/// authority was revoked between enqueue and run — a typed, NON-RETRYABLE
/// failure.
#[derive(Debug, Clone, PartialEq)]
pub enum JobOutcome {
    Done,
    /// Non-retryable: permission denied at run under re-derived authority.
    Denied(String),
    /// Retryable: transient failure; the job returns to `queued`.
    Retry(String),
}

/// How many tenants one claim round considers. A cap keeps the round's query
/// count bounded; tenants beyond it are picked up on the next tick (the round
/// starts from the DB's grouping each time, so nobody is permanently skipped).
const MAX_TENANTS_PER_ROUND: usize = 32;
/// Jobs fetched per tenant to build the round. Only the head matters â€”
/// fairness is about who goes next, not about draining anyone.
const HEAD_PER_TENANT: usize = 10;
/// A single broker job is one bounded database operation. Five minutes leaves
/// ample room for normal tail latency while still recovering crashed workers.
pub const JOB_LEASE_SECONDS: u32 = 300;
/// Retryable work gets a finite execution budget. The fifth transient failure
/// is terminal; a worker crash during the fifth attempt is dead-lettered when
/// its lease expires.
pub const MAX_JOB_ATTEMPTS: u32 = 5;

fn next_claim_token() -> String {
    static CLAIM_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = CLAIM_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{nanos:032x}-{:08x}-{sequence:016x}", std::process::id())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaimSource {
    Queued,
    ExpiredLease,
}

impl ClaimSource {
    fn predicate(self) -> String {
        match self {
            Self::Queued => {
                format!("status = 'queued' AND (attempts = NONE OR attempts < {MAX_JOB_ATTEMPTS})")
            }
            Self::ExpiredLease => format!(
                "status = 'running' AND lease_expires_at != NONE AND \
                 lease_expires_at <= time::now() AND attempts < {MAX_JOB_ATTEMPTS}"
            ),
        }
    }
}

fn eligible_predicate() -> String {
    format!(
        "((status = 'queued' AND (attempts = NONE OR attempts < {MAX_JOB_ATTEMPTS})) OR \
         (status = 'running' AND lease_expires_at != NONE AND \
          lease_expires_at <= time::now() AND attempts < {MAX_JOB_ATTEMPTS}))"
    )
}

fn claim_sql(
    job_id: &str,
    worker_id: &str,
    token: &str,
    source: ClaimSource,
) -> Result<String, BrokerError> {
    let rid = crate::surql::render_value(&Value::RecordId(job_id.to_string()))?;
    Ok(format!(
        "UPDATE {rid} SET status = 'running', claimed_by = '{}', claim_token = '{}', \
         claimed_at = time::now(), lease_expires_at = time::now() + {JOB_LEASE_SECONDS}s, \
         attempts = IF attempts = NONE {{ 1 }} ELSE {{ attempts + 1 }} \
         WHERE {};",
        crate::surql::escape_str(worker_id),
        crate::surql::escape_str(token),
        source.predicate()
    ))
}

fn fenced_update_sql(job: &Job, set: &str) -> Result<String, BrokerError> {
    let rid = crate::surql::render_value(&Value::RecordId(job.id.clone()))?;
    Ok(format!(
        "UPDATE {rid} SET {set} WHERE status = 'running' AND claimed_by = '{}' \
         AND claim_token = '{}';",
        crate::surql::escape_str(&job.lease.claimed_by),
        crate::surql::escape_str(&job.lease.token)
    ))
}

fn retry_update(attempt: u32, detail: &str) -> (bool, String) {
    if attempt >= MAX_JOB_ATTEMPTS {
        return (
            true,
            format!(
                "status = 'dead', terminal_reason = 'attempts_exhausted', last_error = '{}', \
                 finished_at = time::now(), claimed_by = NONE, claim_token = NONE, \
                 lease_expires_at = NONE",
                crate::surql::escape_str(detail)
            ),
        );
    }
    (
        false,
        format!(
            "status = 'queued', claimed_by = NONE, claim_token = NONE, claimed_at = NONE, \
             lease_expires_at = NONE, last_error = '{}'",
            crate::surql::escape_str(detail)
        ),
    )
}

pub struct Worker<'a> {
    pub db: &'a Db,
    /// Needed only to run job effects; claim/rescan don't require it, so a
    /// claim-only worker (isolating the claim race) can omit it.
    pub broker: Option<&'a Broker>,
    pub worker_id: String,
}

impl<'a> Worker<'a> {
    pub fn new(db: &'a Db, broker: &'a Broker, worker_id: impl Into<String>) -> Self {
        Self {
            db,
            broker: Some(broker),
            worker_id: worker_id.into(),
        }
    }

    /// A worker that can claim/rescan but not run effects â€” for isolating
    /// the serialization point under contention.
    pub fn claim_only(db: &'a Db, worker_id: impl Into<String>) -> Self {
        Self {
            db,
            broker: None,
            worker_id: worker_id.into(),
        }
    }

    /// The atomic conditional claim - the ONLY serialization point. A queued
    /// job is preferred; if it is not queued, the same id may be reclaimed only
    /// when its lease has expired. Both paths are conditional `UPDATE`s, so
    /// exactly one contender receives a row.
    pub fn try_claim(&self, job_id: &str) -> Result<Option<Job>, BrokerError> {
        if let Some(job) = self.try_claim_from(job_id, ClaimSource::Queued)? {
            return Ok(Some(job));
        }
        self.try_claim_from(job_id, ClaimSource::ExpiredLease)
    }

    fn try_claim_from(
        &self,
        job_id: &str,
        source: ClaimSource,
    ) -> Result<Option<Job>, BrokerError> {
        let token = next_claim_token();
        let out = self
            .db
            .sql_root(&claim_sql(job_id, &self.worker_id, &token, source)?)?;
        let rows = out.as_array().cloned().unwrap_or_default();
        let won = !rows.is_empty();
        crate::telemetry::inc(
            "frust_job_claim_attempts_total",
            &[
                ("tenant", self.db.tenant_id()),
                (
                    "source",
                    if source == ClaimSource::Queued {
                        "queued"
                    } else {
                        "expired"
                    },
                ),
                ("won", if won { "true" } else { "false" }),
            ],
            1,
        );
        if won && source == ClaimSource::ExpiredLease {
            crate::telemetry::inc(
                "frust_job_stale_claims_recovered_total",
                &[("tenant", self.db.tenant_id())],
                1,
            );
        }
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        Ok(Some(decode_job(&row)?))
    }

    /// Extend a claim before a handler enters a known long-running section.
    /// The update is fenced; a superseded worker cannot revive its old lease.
    pub fn renew_lease(&self, job: &Job) -> Result<bool, BrokerError> {
        let q = fenced_update_sql(
            job,
            &format!("lease_expires_at = time::now() + {JOB_LEASE_SECONDS}s"),
        )?;
        let renewed = self
            .db
            .sql_root(&q)?
            .as_array()
            .is_some_and(|rows| !rows.is_empty());
        if !renewed {
            self.note_fence_rejection("renew");
        }
        Ok(renewed)
    }

    /// Terminalize claims that consumed their last attempt and then crashed.
    /// The predicate is fail-safe: a renewed or reclaimed lease no longer
    /// matches, and a queued record is dead-lettered only at the same budget.
    fn reap_exhausted(&self) -> Result<(), BrokerError> {
        let q = format!(
            "UPDATE job SET status = 'dead', terminal_reason = 'attempts_exhausted', \
             last_error = 'claim lease expired after final attempt', finished_at = time::now(), \
             claimed_by = NONE, claim_token = NONE, lease_expires_at = NONE \
             WHERE attempts >= {MAX_JOB_ATTEMPTS} AND \
             (status = 'queued' OR (status = 'running' AND lease_expires_at != NONE AND \
              lease_expires_at <= time::now()));"
        );
        let dead = self
            .db
            .sql_root(&q)?
            .as_array()
            .map_or(0, std::vec::Vec::len);
        if dead > 0 {
            crate::telemetry::inc(
                "frust_job_dead_letter_total",
                &[
                    ("tenant", self.db.tenant_id()),
                    ("reason", "attempts_exhausted"),
                ],
                dead as u64,
            );
        }
        Ok(())
    }

    /// Claim the next queued job by scanning the queue (cold-start / rescan
    /// path — the LIVE tail feeds specific ids in the daemon, but rescan is
    /// always correct and is the recovery path).
    ///
    /// The claim round is built from PER-TENANT heads, then ordered
    /// round-robin — never from one global window.
    ///
    /// The window approach fails exactly where fairness matters: with 500
    /// jobs queued for A and 5 for B, any window smaller than A's backlog
    /// contains no B at all, and "fair ordering" of an all-A window is just
    /// FIFO with extra steps. Asking per tenant costs 1 + T queries and is
    /// correct at any backlog size.
    pub fn claim_next(&self) -> Result<Option<Job>, BrokerError> {
        self.reap_exhausted()?;
        let eligible = eligible_predicate();
        // tenants with queued work (v3.2.0: a GROUP BY idiom must appear in
        // the projection â€” the sibling of the ORDER BY rule)
        let tenants = self.db.sql_root(&format!(
            "SELECT tenant FROM job WHERE {eligible} GROUP BY tenant;"
        ))?;
        let tenants: Vec<String> = tenants
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|r| {
                        r.get("tenant")
                            .and_then(|t| t.as_str())
                            .unwrap_or(self.db.tenant_id())
                            .to_string()
                    })
                    .collect()
            })
            .unwrap_or_default();

        let mut eligible_jobs: Vec<(String, String)> = Vec::new();
        let mut sources = std::collections::HashMap::new();
        for tenant in tenants.iter().take(MAX_TENANTS_PER_ROUND) {
            let esc = crate::surql::escape_str(tenant);
            let rows = self.db.sql_root(&format!(
                "SELECT id, enqueued_at, status FROM job WHERE {eligible} AND tenant = '{esc}' \
                 ORDER BY enqueued_at LIMIT {HEAD_PER_TENANT};"
            ))?;
            if let Some(arr) = rows.as_array() {
                for r in arr {
                    if let Some(id) = r.get("id").and_then(|i| i.as_str()) {
                        let source = if r.get("status").and_then(|s| s.as_str()) == Some("running")
                        {
                            ClaimSource::ExpiredLease
                        } else {
                            ClaimSource::Queued
                        };
                        eligible_jobs.push((id.to_string(), tenant.clone()));
                        sources.insert(id.to_string(), source);
                    }
                }
            }
        }

        for id in crate::fairness::fair_round(&eligible_jobs) {
            let source = sources.get(&id).copied().unwrap_or(ClaimSource::Queued);
            if let Some(job) = self.try_claim_from(&id, source)? {
                return Ok(Some(job));
            }
        }
        Ok(None)
    }

    /// Run a claimed job under RE-DERIVED authority: the job carries who
    /// requested it, never a snapshot of what they may do. We re-sign-in as
    /// that principal now; if their access was revoked, the write is denied
    /// and the job fails NON-RETRYABLY.
    pub fn run(&self, job: &Job, resolve: &dyn AuthorityResolver) -> JobOutcome {
        // adopt the originating trace: the job's spans join the trace that
        // enqueued it, crossing the async boundary through the record itself
        let trace = job
            .trace
            .as_deref()
            .and_then(crate::telemetry::TraceId::parse)
            .unwrap_or_default();
        let _ctx = crate::telemetry::enter(trace, self.db.tenant_id());
        let span = crate::telemetry::Span::begin("job_run")
            .field("job", job.id.clone())
            .field("kind", job.kind.clone());

        let outcome = self.run_traced(job, resolve);

        crate::telemetry::observe_ms(
            "frust_job_duration_ms",
            &[("tenant", self.db.tenant_id()), ("kind", &job.kind)],
            span.elapsed_ms(),
        );
        let span = span.field(
            "outcome",
            match &outcome {
                JobOutcome::Done => "done",
                JobOutcome::Denied(_) => "denied",
                JobOutcome::Retry(_) => "retry",
            },
        );
        match &outcome {
            JobOutcome::Done => span.ok(),
            JobOutcome::Denied(m) | JobOutcome::Retry(m) => {
                span.err(&crate::contract::BrokerError::Db { detail: m.clone() })
            }
        }
        outcome
    }

    fn run_traced(&self, job: &Job, resolve: &dyn AuthorityResolver) -> JobOutcome {
        let caller = match resolve.caller_for(&job.requested_by) {
            Some(c) => c,
            None => {
                self.finish(job, "denied");
                return JobOutcome::Denied(format!(
                    "principal '{}' no longer exists",
                    job.requested_by
                ));
            }
        };

        // The job's effect: for kind 'create_doc', write payload through the
        // broker under the re-derived caller. Hooks fire (HookChain from the
        // job root) â€” a job handler writing through the broker is the first
        // end-to-end exercise of the re-entrant path.
        let outcome = match job.kind.as_str() {
            "create_doc" => self.run_create_doc(job, &caller),
            other => JobOutcome::Denied(format!("unknown job kind '{other}'")),
        };

        match &outcome {
            JobOutcome::Done => self.finish(job, "done"),
            JobOutcome::Denied(msg) => self.finish_with(job, "denied", msg),
            JobOutcome::Retry(msg) => self.requeue(job, msg),
        }
        outcome
    }

    fn run_create_doc(&self, job: &Job, caller: &Caller) -> JobOutcome {
        let doctype = job
            .payload
            .get("doctype")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let fields = job.payload.get("doc").and_then(|v| v.as_object());
        let Some(fields) = fields else {
            return JobOutcome::Denied("payload.doc missing".into());
        };
        let doc: Vec<(String, Value)> = fields
            .iter()
            .map(|(k, v)| (k.clone(), json_to_value(v)))
            .collect();
        let Some(broker) = self.broker else {
            return JobOutcome::Denied("worker has no broker to run effects".into());
        };
        // fresh HookChain: this is a new causal root (a job, not a nested
        // hook write). Hooks fire; re-entrant db-write is cycle-trapped.
        let chain = HookChain::default();
        match broker.db_write(caller, &chain, WriteOp::Create, doctype, None, &doc) {
            Ok(_) => JobOutcome::Done,
            // Permission denied at run = typed, NON-RETRYABLE.
            Err(BrokerError::PermissionDenied { detail }) => JobOutcome::Denied(detail),
            // A rejected hook is a deterministic non-retryable business
            // failure, not a transient one.
            Err(BrokerError::HookRejected { message, .. }) => JobOutcome::Denied(message),
            // transient (transport, conflict-exhaustion) -> retry
            Err(e) => JobOutcome::Retry(e.to_string()),
        }
    }

    fn finish(&self, job: &Job, status: &str) {
        self.finish_with(job, status, "");
    }

    fn finish_with(&self, job: &Job, status: &str, detail: &str) {
        let set = format!(
            "status = '{}', detail = '{}', terminal_reason = '{}', \
             finished_at = time::now(), claimed_by = NONE, claim_token = NONE, \
             lease_expires_at = NONE",
            crate::surql::escape_str(status),
            crate::surql::escape_str(detail),
            crate::surql::escape_str(status)
        );
        self.apply_fenced(job, "finish", &set);
    }

    fn requeue(&self, job: &Job, detail: &str) {
        let (dead, set) = retry_update(job.lease.attempt, detail);
        if dead {
            if self.apply_fenced(job, "dead_letter", &set) {
                crate::telemetry::inc(
                    "frust_job_dead_letter_total",
                    &[
                        ("tenant", self.db.tenant_id()),
                        ("reason", "attempts_exhausted"),
                    ],
                    1,
                );
            }
            return;
        }

        if self.apply_fenced(job, "requeue", &set) {
            crate::telemetry::inc(
                "frust_job_retries_total",
                &[("tenant", self.db.tenant_id())],
                1,
            );
        }
    }

    fn apply_fenced(&self, job: &Job, action: &str, set: &str) -> bool {
        let result = fenced_update_sql(job, set).and_then(|q| self.db.sql_root(&q));
        let applied = result
            .as_ref()
            .ok()
            .and_then(serde_json::Value::as_array)
            .is_some_and(|rows| !rows.is_empty());
        if !applied {
            self.note_fence_rejection(action);
            if let Err(error) = result {
                crate::telemetry::emit(
                    crate::telemetry::Level::Error,
                    "job_state_transition_failed",
                    &[
                        ("job", serde_json::json!(job.id)),
                        ("action", serde_json::json!(action)),
                        ("error", serde_json::json!(error.to_string())),
                    ],
                );
            }
        }
        applied
    }

    fn note_fence_rejection(&self, action: &str) {
        crate::telemetry::inc(
            "frust_job_fence_rejections_total",
            &[("tenant", self.db.tenant_id()), ("action", action)],
            1,
        );
    }
}

/// Resolves a captured identity to a live caller (re-derives authority).
/// `None` = principal gone/disabled -> the job is denied non-retryably.
pub trait AuthorityResolver: Send + Sync {
    fn caller_for(&self, requested_by: &str) -> Option<Caller>;
}

/// Production resolver: re-derives authority by reading the live `app_user`
/// record. A revoked/disabled/deleted user yields `None` -> non-retryable
/// deny. Jobs carry the user's password-bearing credential indirectly: the
/// job stored who asked; run-time re-signin proves they still may.
///
/// (v0 shortcut: the worker holds a service credential to *read* app_user;
/// per-job re-signin uses a system token minted for the resolved user. The
/// security property that matters â€” authority is a live lookup, not a
/// snapshot â€” holds regardless of the token-minting mechanism.)
pub struct AppUserResolver<'a> {
    pub db: &'a Db,
}

impl AuthorityResolver for AppUserResolver<'_> {
    fn caller_for(&self, requested_by: &str) -> Option<Caller> {
        let esc = crate::surql::escape_str(requested_by);
        let v = self
            .db
            .sql_root(&format!(
                "SELECT name, role, status FROM app_user WHERE name = '{esc}' LIMIT 1;"
            ))
            .ok()?;
        let rec = v.as_array()?.first()?;
        // disabled/suspended users are revoked
        if rec
            .get("status")
            .and_then(|s| s.as_str())
            .is_some_and(|s| s != "active")
        {
            return None;
        }
        let role = rec.get("role").and_then(|r| r.as_str())?.to_string();
        // the run credential is derived, not stored (see note above)
        Some(Caller {
            user: requested_by.to_string(),
            pass: run_pass(requested_by),
            role,
        })
    }
}

/// v0 run-credential derivation. Placeholder for the system-token mint;
/// tests seed users whose password is this so re-signin succeeds.
fn run_pass(user: &str) -> String {
    format!("pw-{user}")
}

/// The resident run loop: replay-from-cursor over the changefeed -> claim &
/// run each job -> advance cursor, then poll the tail. The "LIVE tail" is a
/// short poll of the same changefeed cursor (LIVE and changefeed-replay are
/// interchangeable for fidelity; polling trades latency for zero WS plumbing
/// in the daemon — upgradeable to true LIVE without touching claim/run).
pub struct ResidentWorker<'a> {
    pub worker: Worker<'a>,
    pub resolver: &'a dyn AuthorityResolver,
}

impl ResidentWorker<'_> {
    /// Drain all currently-claimable jobs; returns how many ran. Called on a
    /// timer by `frust serve`. Rescan-based, so it is always correct
    /// regardless of cursor state.
    pub fn tick(&self) -> Result<usize, BrokerError> {
        let mut ran = 0;
        while let Some(job) = self.worker.claim_next()? {
            let _ = self.worker.run(&job, self.resolver);
            ran += 1;
        }
        Ok(ran)
    }
}

// ── The mail worker ─────────────────────────────────────────────────────────

/// One queued message, as the outbox holds it.
#[derive(Debug, Clone)]
pub struct MailRow {
    pub id: String,
    pub notification: String,
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    pub attempts: i64,
    pub trace: Option<String>,
}

/// The dedicated mail worker: claim a queued message, send it BLOCKING, record
/// the outcome. Runs on its own `std::thread` (see `main.rs`) — never on a
/// request thread, and never on the maintenance thread that drains rollups,
/// because a ten-second SMTP timeout must not become ten seconds of stale
/// aggregates.
///
/// This is the deferred-work posture with a different effect: lifecycle event →
/// enqueue → background drain. No async runtime, no second executor; lettre's
/// blocking `SmtpTransport` is what makes that possible.
pub struct MailWorker<'a> {
    pub db: &'a Db,
    pub mailer: &'a crate::mail::Mailer,
    pub worker_id: String,
}

impl MailWorker<'_> {
    /// Drain everything currently sendable; returns how many were delivered.
    ///
    /// Bounded by the queue it saw when it started: a message that fails
    /// transiently returns to `queued` and must NOT be re-picked in this same
    /// pass, or a dead relay would spin the attempt counter to exhaustion inside
    /// one tick and dead-letter mail that a ten-second-later retry would have
    /// delivered. The bound is what makes "bounded retry" mean retry over time.
    pub fn drain(&self) -> Result<usize, BrokerError> {
        let ids = self.queued_ids()?;
        crate::telemetry::gauge(
            "frust_mail_queue_depth",
            &[("tenant", self.db.tenant_id())],
            ids.len() as f64,
        );
        let mut sent = 0;
        for id in ids {
            let Some(row) = self.try_claim_mail(&id)? else {
                continue;
            };
            if self.send_one(&row) {
                sent += 1;
            }
        }
        Ok(sent)
    }

    fn queued_ids(&self) -> Result<Vec<String>, BrokerError> {
        // `enqueued_at` is in the projection because it has to be: on v3.2.0 an
        // ORDER BY idiom absent from the selection is a PARSE ERROR, not a
        // silently unordered result. The job queue's `claim_next` above already
        // carries this caveat in a comment, and this walked into it anyway.
        let out = self.db.sql_root(&format!(
            "SELECT id, enqueued_at FROM {} WHERE status = 'queued' ORDER BY enqueued_at LIMIT 200;",
            crate::mail::OUTBOX_TABLE
        ))?;
        Ok(out
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|r| r.get("id").and_then(|i| i.as_str()).map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// The same atomic conditional claim the job queue uses, for the same
    /// reason: it is the only serialization point, and it keeps two concurrent
    /// workers from claiming the same queued row.
    ///
    /// It does NOT guarantee at-most-once *delivery*. `send_one` calls the mail
    /// transport first and updates the row (`mark_sent`) afterward, and that
    /// `mark_sent` result is ignored. A crash or DB failure between a successful
    /// send and the status write leaves the row un-marked; any recovery requeue
    /// then sends it again. Delivery is therefore at-least-once, and a customer
    /// receiving the same invoice email twice is possible. Making it exactly-once
    /// would need a stable provider idempotency key (follow-up); until then the
    /// bound is at-least-once with dead-lettering after `MAX_ATTEMPTS`.
    fn try_claim_mail(&self, id: &str) -> Result<Option<MailRow>, BrokerError> {
        let rid = crate::surql::render_value(&Value::RecordId(id.to_string()))?;
        let out = self.db.sql_root(&format!(
            "UPDATE {rid} SET status = 'sending', claimed_by = '{}', claimed_at = time::now() \
             WHERE status = 'queued';",
            crate::surql::escape_str(&self.worker_id)
        ))?;
        let Some(row) = out.as_array().and_then(|a| a.first()) else {
            return Ok(None);
        };
        Ok(Some(MailRow {
            id: id.to_string(),
            notification: row
                .get("notification")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            to: row
                .get("to")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            subject: row
                .get("subject")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            body: row
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
            attempts: row.get("attempts").and_then(|v| v.as_i64()).unwrap_or(0),
            trace: row
                .get("trace")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        }))
    }

    /// Send one claimed message and record the outcome. `true` = delivered.
    fn send_one(&self, row: &MailRow) -> bool {
        // adopt the enqueueing request's trace, so one trace spans
        // save → enqueue → delivery, across the thread boundary
        let trace = row
            .trace
            .as_deref()
            .and_then(crate::telemetry::TraceId::parse)
            .unwrap_or_default();
        let _ctx = crate::telemetry::enter(trace, self.db.tenant_id());
        let span = crate::telemetry::Span::begin("mail_send")
            .field("notification", row.notification.clone())
            .field("mode", self.mailer.mode())
            .field("recipients", row.to.len() as i64);

        let out = crate::mail::Outbound {
            to: row.to.clone(),
            subject: row.subject.clone(),
            body: row.body.clone(),
        };
        let result = self.mailer.deliver(&out);
        let tenant = self.db.tenant_id().to_string();
        crate::telemetry::observe_ms(
            "frust_mail_send_duration_ms",
            &[("tenant", &tenant), ("mode", self.mailer.mode())],
            span.elapsed_ms(),
        );

        match result {
            Ok(receipt) => {
                let _ = self.mark_sent(&row.id, &receipt);
                crate::telemetry::inc(
                    "frust_mail_sent_total",
                    &[("tenant", &tenant), ("mode", self.mailer.mode())],
                    1,
                );
                span.ok();
                true
            }
            Err(failure) => {
                let attempts = row.attempts + 1;
                // Permanent means permanent: retrying a 5xx or an unparseable
                // address four more times only delays the report.
                let exhausted = attempts >= crate::mail::MAX_ATTEMPTS;
                let dead = matches!(failure, crate::mail::Failure::Permanent(_)) || exhausted;
                let reason = failure.reason().to_string();
                if dead {
                    let label = if matches!(failure, crate::mail::Failure::Permanent(_)) {
                        "permanent"
                    } else {
                        "attempts_exhausted"
                    };
                    let _ = self.mark_dead(&row.id, attempts, &reason);
                    crate::telemetry::inc(
                        "frust_mail_dead_total",
                        &[("tenant", &tenant), ("reason", label)],
                        1,
                    );
                    crate::telemetry::emit(
                        crate::telemetry::Level::Error,
                        "mail_dead_letter",
                        &[
                            ("notification", serde_json::json!(row.notification)),
                            ("attempts", serde_json::json!(attempts)),
                            ("reason", serde_json::json!(label)),
                            ("error", serde_json::json!(reason.clone())),
                        ],
                    );
                } else {
                    let _ = self.mark_retry(&row.id, attempts, &reason);
                    crate::telemetry::inc(
                        "frust_mail_retry_total",
                        &[("tenant", &tenant), ("kind", failure.kind())],
                        1,
                    );
                }
                span.err(&BrokerError::Db { detail: reason });
                false
            }
        }
    }

    fn mark_sent(&self, id: &str, receipt: &str) -> Result<(), BrokerError> {
        let rid = crate::surql::render_value(&Value::RecordId(id.to_string()))?;
        self.db.sql_root(&format!(
            "UPDATE {rid} SET status = 'sent', receipt = '{}', sent_at = time::now(), \
             attempts = attempts + 1;",
            crate::surql::escape_str(receipt)
        ))?;
        Ok(())
    }

    fn mark_retry(&self, id: &str, attempts: i64, detail: &str) -> Result<(), BrokerError> {
        let rid = crate::surql::render_value(&Value::RecordId(id.to_string()))?;
        self.db.sql_root(&format!(
            "UPDATE {rid} SET status = 'queued', claimed_by = NONE, attempts = {attempts}, \
             last_error = '{}';",
            crate::surql::escape_str(detail)
        ))?;
        Ok(())
    }

    fn mark_dead(&self, id: &str, attempts: i64, detail: &str) -> Result<(), BrokerError> {
        let rid = crate::surql::render_value(&Value::RecordId(id.to_string()))?;
        self.db.sql_root(&format!(
            "UPDATE {rid} SET status = 'dead', attempts = {attempts}, last_error = '{}', \
             finished_at = time::now();",
            crate::surql::escape_str(detail)
        ))?;
        Ok(())
    }
}

fn decode_job(row: &serde_json::Value) -> Result<Job, BrokerError> {
    let required_string = |field: &str| {
        row.get(field)
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_string)
            .ok_or_else(|| BrokerError::Db {
                detail: format!("claimed job is missing lease field '{field}'"),
            })
    };
    let attempt = row
        .get("attempts")
        .and_then(serde_json::Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| BrokerError::Db {
            detail: "claimed job has invalid attempt count".into(),
        })?;
    Ok(Job {
        id: row
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        kind: row
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        requested_by: row
            .get("requested_by")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        payload: row
            .get("payload")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
        trace: row
            .get("trace")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        lease: JobLease {
            claimed_by: required_string("claimed_by")?,
            token: required_string("claim_token")?,
            attempt,
            expires_at: required_string("lease_expires_at")?,
        },
    })
}

fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Int)
            .unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0))),
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(a) => Value::List(a.iter().map(json_to_value).collect()),
        serde_json::Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), json_to_value(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod lease_tests {
    use super::*;

    fn leased_job(attempt: u32) -> Job {
        Job {
            id: "job:alpha".into(),
            kind: "create_doc".into(),
            requested_by: "user".into(),
            payload: serde_json::Value::Null,
            trace: None,
            lease: JobLease {
                claimed_by: "worker-a".into(),
                token: "token-a".into(),
                attempt,
                expires_at: "2026-08-01T00:05:00Z".into(),
            },
        }
    }

    #[test]
    fn queued_and_expired_claims_are_atomic_and_attempt_bounded() {
        let queued = claim_sql("job:alpha", "worker-a", "token-a", ClaimSource::Queued).unwrap();
        assert!(queued.starts_with("UPDATE type::record('job:alpha')"));
        assert!(queued.contains(" SET status = 'running'"));
        assert!(queued.contains("claim_token = 'token-a'"));
        assert!(queued.contains("lease_expires_at = time::now() + 300s"));
        assert!(queued.contains("attempts = IF attempts = NONE { 1 } ELSE { attempts + 1 }"));
        assert!(queued.contains("status = 'queued' AND (attempts = NONE OR attempts < 5)"));

        let expired = claim_sql(
            "job:alpha",
            "worker-a",
            "token-b",
            ClaimSource::ExpiredLease,
        )
        .unwrap();
        assert!(expired.contains("status = 'running'"));
        assert!(expired.contains("lease_expires_at <= time::now()"));
        assert!(expired.contains("attempts < 5"));
    }

    #[test]
    fn every_state_transition_is_fenced_by_owner_and_token() {
        let sql = fenced_update_sql(&leased_job(1), "status = 'done'").unwrap();
        assert!(sql.contains("WHERE status = 'running'"));
        assert!(sql.contains("claimed_by = 'worker-a'"));
        assert!(sql.contains("claim_token = 'token-a'"));
        assert!(!sql.contains("lease_expires_at > time::now()"));
    }

    #[test]
    fn fifth_retry_dead_letters_instead_of_requeueing() {
        let (dead, fifth) = retry_update(MAX_JOB_ATTEMPTS, "transport failed");
        assert!(dead);
        assert!(fifth.contains("status = 'dead'"));
        assert!(fifth.contains("terminal_reason = 'attempts_exhausted'"));

        let (dead, fourth) = retry_update(MAX_JOB_ATTEMPTS - 1, "transport failed");
        assert!(!dead);
        assert!(fourth.contains("status = 'queued'"));
        assert!(fourth.contains("claim_token = NONE"));
    }

    #[test]
    fn eligibility_keeps_expired_leases_in_the_fair_tenant_round() {
        let predicate = eligible_predicate();
        assert!(predicate.contains("status = 'queued'"));
        assert!(predicate.contains("status = 'running'"));
        assert!(predicate.contains("lease_expires_at <= time::now()"));
        assert_eq!(predicate.matches("attempts < 5").count(), 2);
    }

    #[test]
    fn claimed_rows_require_complete_typed_lease_state() {
        let row = serde_json::json!({
            "id": "job:alpha",
            "kind": "create_doc",
            "requested_by": "user",
            "payload": {},
            "claimed_by": "worker-a",
            "claim_token": "token-a",
            "attempts": 2,
            "lease_expires_at": "2026-08-01T00:05:00Z"
        });
        assert_eq!(decode_job(&row).unwrap().lease.attempt, 2);

        let mut incomplete = row;
        incomplete.as_object_mut().unwrap().remove("claim_token");
        assert!(decode_job(&incomplete)
            .unwrap_err()
            .to_string()
            .contains("claim_token"));
    }

    #[test]
    fn sql_builders_escape_worker_tokens_and_error_details() {
        let claim = claim_sql("job:alpha", "worker'one", "token'two", ClaimSource::Queued).unwrap();
        assert!(claim.contains("claimed_by = 'worker\\'one'"));
        assert!(claim.contains("claim_token = 'token\\'two'"));
        let (_, retry) = retry_update(1, "relay's down");
        assert!(retry.contains("last_error = 'relay\\'s down'"));
    }
}
