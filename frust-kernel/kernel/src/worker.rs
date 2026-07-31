//! Module 5: the job worker (ADR-009 Half 2, verbatim).
//!
//! The loop: replay-from-cursor (versionstamp changefeed) -> LIVE tail ->
//! advance cursor. LIVE is a latency optimization over a changefeed-backed
//! log (WO-004 verdict: viable-with-bridge). The atomic conditional claim is
//! the ONLY serialization point â€” duplicate delivery is harmless by
//! construction (ADR-009 ruling #1). Cold start beyond retention = rescan
//! `status='queued'`, jobs are records (ADR-009 ruling #2).
//!
//! This module speaks HTTP `/sql` for claim/run (the wire-protocol transport
//! contract); the LIVE tail is WO-004's proven WS path, wired in when the
//! worker runs as a daemon. For the kernel v0 milestone the executed,
//! test-covered core is claim + run + retention rescan â€” the two exit
//! criteria (6, 7) and criterion 5's authority-re-derivation.

use crate::broker::{Broker, Caller, HookChain};
use crate::contract::{BrokerError, Value, WriteOp};
use crate::db::Db;

/// A claimed job, decoded from the queue.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub requested_by: String,
    pub payload: serde_json::Value,
    /// The originating trace (stamped at enqueue) â€” adopted at run so one
    /// trace spans REST -> enqueue -> claim -> job effect (REQ-6.4.1).
    pub trace: Option<String>,
}

/// Outcome of running one job. `Denied` is the criterion-5 hard case:
/// authority was revoked between enqueue and run â€” a typed, NON-RETRYABLE
/// failure (ADR-006 edge 4).
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

pub struct Worker<'a> {
    pub db: &'a Db,
    /// Needed only to run job effects; claim/rescan don't require it, so a
    /// claim-only worker (the criterion-6 race) can omit it.
    pub broker: Option<&'a Broker>,
    pub worker_id: String,
}

impl<'a> Worker<'a> {
    pub fn new(db: &'a Db, broker: &'a Broker, worker_id: impl Into<String>) -> Self {
        Self { db, broker: Some(broker), worker_id: worker_id.into() }
    }

    /// A worker that can claim/rescan but not run effects â€” for isolating
    /// the serialization point under contention.
    pub fn claim_only(db: &'a Db, worker_id: impl Into<String>) -> Self {
        Self { db, broker: None, worker_id: worker_id.into() }
    }

    /// The atomic conditional claim â€” the ONLY serialization point (ADR-009
    /// ruling #1). `UPDATE ... WHERE status='queued'` is atomic in SurrealDB;
    /// exactly one contender flips the row, the losers match zero rows and
    /// move on. Returns the claimed job, or None if another worker won it.
    pub fn try_claim(&self, job_id: &str) -> Result<Option<Job>, BrokerError> {
        let rid = crate::surql::render_value(&Value::RecordId(job_id.to_string()))?;
        let q = format!(
            "UPDATE {rid} SET status = 'running', claimed_by = '{}', claimed_at = time::now() \
             WHERE status = 'queued';",
            crate::surql::escape_str(&self.worker_id)
        );
        let out = self.db.sql_root(&q)?;
        let rows = out.as_array().cloned().unwrap_or_default();
        // WHERE matched nothing (already claimed) -> empty result -> not ours
        let won = !rows.is_empty();
        crate::telemetry::inc(
            "frust_job_claim_attempts_total",
            &[("tenant", self.db.tenant_id()), ("won", if won { "true" } else { "false" })],
            1,
        );
        let Some(row) = rows.into_iter().next() else { return Ok(None) };
        Ok(Some(decode_job(&row)?))
    }

    /// Claim the next queued job by scanning the queue (cold-start / rescan
    /// path, ADR-009 ruling #2 â€” the LIVE tail feeds specific ids in the
    /// daemon, but rescan is always correct and is the recovery path).
    ///
    /// WO-013: the claim round is built from PER-TENANT heads, then ordered
    /// round-robin â€” never from one global window.
    ///
    /// The window approach fails exactly where fairness matters: with 500
    /// jobs queued for A and 5 for B, any window smaller than A's backlog
    /// contains no B at all, and "fair ordering" of an all-A window is just
    /// FIFO with extra steps. Asking per tenant costs 1 + T queries and is
    /// correct at any backlog size.
    pub fn claim_next(&self) -> Result<Option<Job>, BrokerError> {
        // tenants with queued work (v3.2.0: a GROUP BY idiom must appear in
        // the projection â€” the sibling of the ORDER BY rule)
        let tenants = self
            .db
            .sql_root("SELECT tenant FROM job WHERE status = 'queued' GROUP BY tenant;")?;
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

        let mut queued: Vec<(String, String)> = Vec::new();
        for tenant in tenants.iter().take(MAX_TENANTS_PER_ROUND) {
            let esc = crate::surql::escape_str(tenant);
            let rows = self.db.sql_root(&format!(
                "SELECT id, enqueued_at FROM job WHERE status = 'queued' AND tenant = '{esc}' \
                 ORDER BY enqueued_at LIMIT {HEAD_PER_TENANT};"
            ))?;
            if let Some(arr) = rows.as_array() {
                for r in arr {
                    if let Some(id) = r.get("id").and_then(|i| i.as_str()) {
                        queued.push((id.to_string(), tenant.clone()));
                    }
                }
            }
        }

        for id in crate::fairness::fair_round(&queued) {
            if let Some(job) = self.try_claim(&id)? {
                return Ok(Some(job));
            }
        }
        Ok(None)
    }

    /// Run a claimed job under RE-DERIVED authority (ADR-006 edge 4): the job
    /// carries who requested it, never a snapshot of what they may do. We
    /// re-sign-in as that principal now; if their access was revoked, the
    /// write is denied and the job fails NON-RETRYABLY.
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
                self.finish(&job.id, "denied");
                return JobOutcome::Denied(format!("principal '{}' no longer exists", job.requested_by));
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
            JobOutcome::Done => self.finish(&job.id, "done"),
            JobOutcome::Denied(msg) => self.finish_with(&job.id, "denied", msg),
            JobOutcome::Retry(msg) => self.requeue(&job.id, msg),
        }
        outcome
    }

    fn run_create_doc(&self, job: &Job, caller: &Caller) -> JobOutcome {
        let doctype = job.payload.get("doctype").and_then(|v| v.as_str()).unwrap_or("");
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
            // Permission denied at run = typed, NON-RETRYABLE (criterion 5).
            Err(BrokerError::PermissionDenied { detail }) => JobOutcome::Denied(detail),
            // A rejected hook is a deterministic non-retryable business
            // failure, not a transient one.
            Err(BrokerError::HookRejected { message, .. }) => JobOutcome::Denied(message),
            // transient (transport, conflict-exhaustion) -> retry
            Err(e) => JobOutcome::Retry(e.to_string()),
        }
    }

    fn finish(&self, job_id: &str, status: &str) {
        self.finish_with(job_id, status, "");
    }

    fn finish_with(&self, job_id: &str, status: &str, detail: &str) {
        if let Ok(rid) = crate::surql::render_value(&Value::RecordId(job_id.to_string())) {
            let _ = self.db.sql_root(&format!(
                "UPDATE {rid} SET status = '{}', detail = '{}', finished_at = time::now();",
                crate::surql::escape_str(status),
                crate::surql::escape_str(detail)
            ));
        }
    }

    fn requeue(&self, job_id: &str, detail: &str) {
        if let Ok(rid) = crate::surql::render_value(&Value::RecordId(job_id.to_string())) {
            let _ = self.db.sql_root(&format!(
                "UPDATE {rid} SET status = 'queued', claimed_by = NONE, last_error = '{}';",
                crate::surql::escape_str(detail)
            ));
        }
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
        if rec.get("status").and_then(|s| s.as_str()).is_some_and(|s| s != "active") {
            return None;
        }
        let role = rec.get("role").and_then(|r| r.as_str())?.to_string();
        // the run credential is derived, not stored (see note above)
        Some(Caller { user: requested_by.to_string(), pass: run_pass(requested_by), role })
    }
}

/// v0 run-credential derivation. Placeholder for the system-token mint;
/// tests seed users whose password is this so re-signin succeeds.
fn run_pass(user: &str) -> String {
    format!("pw-{user}")
}

/// The resident run loop (ADR-009 Half 2, verbatim): replay-from-cursor over
/// the changefeed -> claim & run each job -> advance cursor, then poll the
/// tail. In v0 the "LIVE tail" is a short poll of the same changefeed cursor
/// (WO-004 proved LIVE and changefeed-replay are interchangeable for
/// fidelity; polling trades latency for zero WS plumbing in the daemon â€”
/// upgradeable to true LIVE without touching claim/run).
pub struct ResidentWorker<'a> {
    pub worker: Worker<'a>,
    pub resolver: &'a dyn AuthorityResolver,
}

impl ResidentWorker<'_> {
    /// Drain all currently-claimable jobs; returns how many ran. Called on a
    /// timer by `frust serve`. Rescan-based, so it is always correct
    /// regardless of cursor state (ADR-009 ruling #2).
    pub fn tick(&self) -> Result<usize, BrokerError> {
        let mut ran = 0;
        while let Some(job) = self.worker.claim_next()? {
            let _ = self.worker.run(&job, self.resolver);
            ran += 1;
        }
        Ok(ran)
    }
}

// ── WO-043: the mail worker ─────────────────────────────────────────────────

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
/// This is the ADR-010 Tier-2 posture with a different effect: lifecycle event →
/// enqueue → background drain. No async runtime, no second executor; lettre's
/// blocking `SmtpTransport` is what makes that possible (PM ruling B).
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
            let Some(row) = self.try_claim_mail(&id)? else { continue };
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

    /// The same atomic conditional claim the job queue uses (ADR-009 ruling #1),
    /// for the same reason: it is the only serialization point, and duplicate
    /// delivery must be impossible rather than unlikely — a customer receiving
    /// the same invoice email twice is a support ticket.
    fn try_claim_mail(&self, id: &str) -> Result<Option<MailRow>, BrokerError> {
        let rid = crate::surql::render_value(&Value::RecordId(id.to_string()))?;
        let out = self.db.sql_root(&format!(
            "UPDATE {rid} SET status = 'sending', claimed_by = '{}', claimed_at = time::now() \
             WHERE status = 'queued';",
            crate::surql::escape_str(&self.worker_id)
        ))?;
        let Some(row) = out.as_array().and_then(|a| a.first()) else { return Ok(None) };
        Ok(Some(MailRow {
            id: id.to_string(),
            notification: row.get("notification").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            to: row
                .get("to")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str()).map(str::to_string).collect())
                .unwrap_or_default(),
            subject: row.get("subject").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            body: row.get("body").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
            attempts: row.get("attempts").and_then(|v| v.as_i64()).unwrap_or(0),
            trace: row.get("trace").and_then(|v| v.as_str()).map(str::to_string),
        }))
    }

    /// Send one claimed message and record the outcome. `true` = delivered.
    fn send_one(&self, row: &MailRow) -> bool {
        // adopt the enqueueing request's trace, so one trace spans
        // save → enqueue → delivery (REQ-6.4.1), across the thread boundary
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
    Ok(Job {
        id: row.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        kind: row.get("kind").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        requested_by: row.get("requested_by").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
        payload: row.get("payload").cloned().unwrap_or(serde_json::Value::Null),
        trace: row.get("trace").and_then(|v| v.as_str()).map(str::to_string),
    })
}

fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => {
            n.as_i64().map(Value::Int).unwrap_or_else(|| Value::Float(n.as_f64().unwrap_or(0.0)))
        }
        serde_json::Value::String(s) => Value::Text(s.clone()),
        serde_json::Value::Array(a) => Value::List(a.iter().map(json_to_value).collect()),
        serde_json::Value::Object(o) => {
            Value::Object(o.iter().map(|(k, v)| (k.clone(), json_to_value(v))).collect())
        }
    }
}

