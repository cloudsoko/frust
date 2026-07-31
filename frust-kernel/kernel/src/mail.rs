//! WO-043: email batteries — notifications as metadata, delivery as a
//! contained background side-effect.
//!
//! **Transport: lettre directly, blocking (PM ruling, 2026-07-30).** Not
//! `topcoat-mail`: its `Transport::send(&self, cx: &Cx, mail)` needs a Topcoat
//! *web request context* to render `Mail::html` as a `View`, so wiring it would
//! pull `topcoat-core` + `topcoat-view` into a kernel that ADR-004 keeps
//! Topcoat-free, and would make the kernel synthesise a fake request to satisfy
//! a signature. lettre's `SmtpTransport` is blocking and ungated, so this module
//! adds no async runtime and no tokio to the mail path (tokio is in the graph
//! only via `wasmtime-wasi`, and stays there).
//!
//! **This module contains no query text**, by design rather than by accident:
//! the outbox claim/finish lives in `worker.rs`, the notification lookup in
//! `sync.rs`, and the enqueue in `broker.rs` — the three modules
//! `surql_monopoly` already sanctions for exactly those roles. Widening the
//! allowlist would have been the easier move and the wrong one.
//!
//! What lives here: config selection, the transport, the notification rule
//! shape, event matching, condition evaluation, field interpolation, and
//! recipient parsing. All of it pure enough to unit-test without a database.

use std::path::PathBuf;

use lettre::message::{header::ContentType, Mailbox};
use lettre::transport::smtp::SmtpTransport;
use lettre::{FileTransport, Message, Transport};
use serde::{Deserialize, Serialize};

use crate::contract::BrokerError;
use crate::sync::Rule;

/// The delivery outbox. Kernel-owned meta (see `meta::mail_ddl`).
pub const OUTBOX_TABLE: &str = "_frust_mail";
/// The notification rule table — metadata, like `workflow`.
pub const NOTIFICATION_TABLE: &str = "notification";

/// How many delivery attempts before a message dead-letters.
///
/// Bounded on purpose (criterion 4). The job queue's `requeue` retries a
/// transient failure forever, which for mail means an SMTP outage turns into an
/// unbounded loop that hides the outage instead of reporting it.
pub const MAX_ATTEMPTS: i64 = 5;

/// The closed set of lifecycle events a notification may declare.
///
/// Closed because an unrecognised event string would produce a notification
/// that never fires and never complains — the exact silent failure criterion 4
/// exists to refuse. `NotificationDef::validate` rejects anything else at the
/// door, so a typo is a 400 at write time, not a mystery six months later.
pub const EVENTS: [&str; 5] =
    ["after_insert", "on_update", "on_transition", "on_submit", "on_cancel"];

// ── config: file or smtp, by config, never by code ──────────────────────────

/// Which transport the process delivers through. Selected from `FRUST_MAIL`
/// exactly once, at boot (the tenancy-strategy pattern).
pub enum Backend {
    File(FileTransport),
    Smtp(SmtpTransport),
}

pub struct Mailer {
    backend: Backend,
    from: Mailbox,
    /// `"file"` / `"smtp"` — a metric label and a boot-log field, so the mode
    /// in force is never something an operator has to infer.
    mode: &'static str,
    /// The directory or relay in force, for the same reason.
    target: String,
}

/// Hand-written, like `Tenancy`'s and for the same reason: the mode and target
/// are what an operator needs, and a derived `Debug` would print whatever
/// credentials a future `FRUST_MAIL_USER`/`_PASS` adds straight into a log line.
impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mailer").field("mode", &self.mode).field("target", &self.target).finish()
    }
}

/// A delivery failure, classified — because retryable and hopeless need
/// different handling, and guessing means either losing mail or looping on it.
#[derive(Debug)]
pub enum Failure {
    /// Try again later, up to `MAX_ATTEMPTS` (SMTP 4xx, connection refused,
    /// timeout, a full disk).
    Transient(String),
    /// Never going to work: dead-letter now (SMTP 5xx, an unparseable address,
    /// a template referring to a field that does not exist).
    Permanent(String),
}

impl Failure {
    pub fn reason(&self) -> &str {
        match self {
            Failure::Transient(m) | Failure::Permanent(m) => m,
        }
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Failure::Transient(_) => "transient",
            Failure::Permanent(_) => "permanent",
        }
    }
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

impl Mailer {
    /// Build the process mailer from the environment.
    ///
    /// `FRUST_MAIL=file` (default) writes `.eml` + envelope `.json` to
    /// `FRUST_MAIL_DIR`; `FRUST_MAIL=smtp` relays through `FRUST_MAIL_SMTP`
    /// (`host` or `host:port`). Anything else **refuses** rather than falling
    /// back — a typo'd transport name that quietly wrote mail to a directory
    /// nobody reads is the silent-misbehaviour class, and ADR-008's fail-closed
    /// posture applies to delivery as much as to schema.
    ///
    /// The file directory is created here, not on first send: a missing
    /// directory would otherwise surface as a per-message transient failure five
    /// times over, when it is really one configuration mistake.
    pub fn from_env() -> Result<Self, BrokerError> {
        let refuse = |detail: String| BrokerError::InvalidValue { detail };
        let from_raw = env_or("FRUST_MAIL_FROM", "frust@localhost");
        let from: Mailbox = from_raw
            .parse()
            .map_err(|e| refuse(format!("FRUST_MAIL_FROM {from_raw:?} is not an address: {e}")))?;

        match env_or("FRUST_MAIL", "file").as_str() {
            "file" => Self::file(env_or("FRUST_MAIL_DIR", "mail-outbox"), from),
            "smtp" => Self::smtp(env_or("FRUST_MAIL_SMTP", "127.0.0.1:25"), from),
            other => Err(refuse(format!(
                "FRUST_MAIL must be 'file' or 'smtp', got {other:?} — refusing rather than \
                 guessing which transport you meant"
            ))),
        }
    }

    /// The file transport, explicitly. Split out from `from_env` so tests can
    /// build a mailer without touching process-global environment variables —
    /// cargo runs test threads in ONE process, so an env-var-only constructor
    /// would make every mail test race every other one.
    pub fn file(dir: impl Into<PathBuf>, from: Mailbox) -> Result<Self, BrokerError> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).map_err(|e| BrokerError::InvalidValue {
            detail: format!("mail directory {} is not usable: {e}", dir.display()),
        })?;
        let target = dir.display().to_string();
        Ok(Self { backend: Backend::File(FileTransport::with_envelope(dir)), from, mode: "file", target })
    }

    /// The SMTP transport, explicitly. `relay` is `host` or `host:port`.
    pub fn smtp(relay: impl Into<String>, from: Mailbox) -> Result<Self, BrokerError> {
        let relay = relay.into();
        let (host, port) = match relay.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>().map_err(|_| BrokerError::InvalidValue {
                    detail: format!("SMTP port {p:?} is not a port"),
                })?,
            ),
            None => (relay.clone(), 25u16),
        };
        // `builder_dangerous` = plaintext/opportunistic, which is what a local
        // relay or a container-network MTA actually is. TLS is available (the
        // `rustls-tls` feature is compiled in) and is the next config knob;
        // naming it "dangerous" in the source is lettre being honest and this
        // comment keeping it honest.
        // ponytail: plaintext relay; add FRUST_MAIL_TLS=starttls when a
        // deployment relays to a public submission port.
        let smtp = SmtpTransport::builder_dangerous(host)
            .port(port)
            // A dead relay must slow EMAIL, never a save — the worker thread is
            // what absorbs this wait, and bounding it keeps the outbox draining
            // instead of parking forever on one message.
            .timeout(Some(std::time::Duration::from_secs(10)))
            .build();
        Ok(Self { backend: Backend::Smtp(smtp), from, mode: "smtp", target: relay })
    }

    /// The configured sender address — tests and the boot log both want it.
    pub fn from_address(&self) -> String {
        self.from.to_string()
    }

    pub fn mode(&self) -> &'static str {
        self.mode
    }
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Deliver one message, blocking. Returns the transport's receipt id.
    pub fn deliver(&self, out: &Outbound) -> Result<String, Failure> {
        let mut builder = Message::builder().from(self.from.clone()).subject(&out.subject);
        for addr in &out.to {
            let mbox: Mailbox = addr
                .parse()
                .map_err(|e| Failure::Permanent(format!("recipient {addr:?}: {e}")))?;
            builder = builder.to(mbox);
        }
        let msg = builder
            .header(ContentType::TEXT_PLAIN)
            .body(out.body.clone())
            .map_err(|e| Failure::Permanent(format!("message build: {e}")))?;

        match &self.backend {
            Backend::File(t) => {
                t.send(&msg).map(|id| id.to_string()).map_err(|e| Failure::Transient(e.to_string()))
            }
            Backend::Smtp(t) => t.send(&msg).map(|r| r.code().to_string()).map_err(|e| {
                // lettre classifies its own SMTP failures; trusting it beats a
                // hand-rolled 4xx/5xx guess that would be wrong on timeouts.
                let m = e.to_string();
                if e.is_permanent() {
                    Failure::Permanent(m)
                } else {
                    Failure::Transient(m)
                }
            }),
        }
    }
}

/// A message resolved down to concrete addresses and rendered text — what the
/// outbox row holds, and all the worker needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Outbound {
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
}

// ── the notification rule: metadata, not code ───────────────────────────────

fn default_true() -> bool {
    true
}

/// One notification rule. Stored as a record in `notification`, so adding one
/// is a write, not a build (criterion 1).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NotificationDef {
    pub name: String,
    /// The DocType whose lifecycle this watches.
    pub doctype: String,
    /// One of `EVENTS`.
    pub event: String,
    /// `on_transition` only: fire for this workflow action. Absent = any action.
    #[serde(default)]
    pub action: Option<String>,
    /// Optional gate, in the SAME vocabulary WO-014 already defined for client
    /// rules (`sync::Rule`). Reusing it is deliberate: a second condition
    /// dialect would be a second thing to learn and a second thing to get
    /// wrong, and the WO explicitly forbids a new template/rule language.
    #[serde(default)]
    pub condition: Option<Rule>,
    /// `role:<role>` · `field:<fieldname>` · a literal address.
    pub recipients: Vec<String>,
    pub subject: String,
    pub body: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

/// What just happened to a document. Built at the fire site, never stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Trigger<'a> {
    AfterInsert,
    Update,
    Transition { action: &'a str, to_state: &'a str },
    /// docstatus crossed into 1 (ADR-009's submit edge).
    Submit,
    /// docstatus crossed into 2.
    Cancel,
}

impl Trigger<'_> {
    fn event(&self) -> &'static str {
        match self {
            Trigger::AfterInsert => "after_insert",
            Trigger::Update => "on_update",
            Trigger::Transition { .. } => "on_transition",
            Trigger::Submit => "on_submit",
            Trigger::Cancel => "on_cancel",
        }
    }
}

impl NotificationDef {
    /// Problems with this rule, in operator prose. Non-empty = refuse the write.
    ///
    /// Validating at the door is the whole reason a notification can be trusted
    /// to fire: a rule that stores successfully and then silently matches
    /// nothing is indistinguishable from a broken mail transport, and an
    /// operator would go looking in the wrong place.
    pub fn validate(&self) -> Vec<String> {
        let mut bad = Vec::new();
        let ident = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ident(&self.name) {
            bad.push(format!("name {:?} must be a plain identifier", self.name));
        }
        if !ident(&self.doctype) {
            bad.push(format!("doctype {:?} must be a plain identifier", self.doctype));
        }
        if !EVENTS.contains(&self.event.as_str()) {
            bad.push(format!("event {:?} is not one of {EVENTS:?}", self.event));
        }
        if self.action.is_some() && self.event != "on_transition" {
            bad.push(format!("'action' only applies to on_transition, not {:?}", self.event));
        }
        if self.recipients.is_empty() {
            bad.push("recipients is empty — a notification with nobody to send to never sends".into());
        }
        for r in &self.recipients {
            if let Err(e) = Recipient::parse(r) {
                bad.push(e);
            }
        }
        if self.subject.trim().is_empty() {
            bad.push("subject is empty".into());
        }
        if let Some(c) = &self.condition {
            if !RULE_OPS.contains(&c.op.as_str()) {
                bad.push(format!("condition op {:?} is not one of {RULE_OPS:?}", c.op));
            }
        }
        bad
    }

    /// Does this rule fire for this event on this document?
    pub fn matches(&self, trigger: &Trigger, doc: &serde_json::Value) -> bool {
        if !self.enabled || self.event != trigger.event() {
            return false;
        }
        if let Trigger::Transition { action, .. } = trigger {
            if self.action.as_deref().is_some_and(|want| want != *action) {
                return false;
            }
        }
        self.condition.as_ref().is_none_or(|c| rule_holds(c, doc))
    }
}

// ── conditions: comparisons only, money compared never computed ─────────────

pub const RULE_OPS: [&str; 8] = ["eq", "ne", "gt", "lt", "ge", "le", "empty", "not_empty"];

/// Evaluate a `sync::Rule` against a document.
///
/// Numeric comparison goes through `Decimal`, so `total ge 1000` on a money
/// field compares exact stored decimals — never a float, and never arithmetic
/// (ADR-007, and the WO's own boundary). Values that are not both numeric fall
/// back to string comparison, which is what `eq` on a status field wants.
pub fn rule_holds(rule: &Rule, doc: &serde_json::Value) -> bool {
    use crate::decimal::Decimal;
    let field = doc.get(&rule.field);
    let text = field.map(render_field).unwrap_or_default();
    let want = rule.value.clone().unwrap_or_default();
    match rule.op.as_str() {
        "empty" => text.is_empty(),
        "not_empty" => !text.is_empty(),
        op => {
            // `Decimal` is `Ord` (its `PartialEq` delegates to `cmp`, so the two
            // cannot disagree — see decimal.rs), which is what makes a money
            // condition exact rather than approximately right.
            let ord = match (Decimal::parse(&text), Decimal::parse(&want)) {
                (Some(a), Some(b)) => Some(a.cmp(&b)),
                _ => Some(text.as_str().cmp(want.as_str())),
            };
            match (op, ord) {
                ("eq", Some(o)) => o.is_eq(),
                ("ne", Some(o)) => o.is_ne(),
                ("gt", Some(o)) => o.is_gt(),
                ("lt", Some(o)) => o.is_lt(),
                ("ge", Some(o)) => o.is_ge(),
                ("le", Some(o)) => o.is_le(),
                _ => false,
            }
        }
    }
}

// ── interpolation: `{{field}}`, and an unknown field is an ERROR ────────────

/// One field's value as template text.
///
/// A stored decimal arrives from SurrealDB as a JSON **string** and is rendered
/// verbatim — the ADR-007 compare-never-compute rule means a template shows
/// what is stored, with no reformatting and no arithmetic. A number renders
/// through its own JSON form for the same reason.
pub fn render_field(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

/// Substitute `{{field}}` from the document.
///
/// **An unknown field is an error, not an empty string.** Rendering `{{totl}}`
/// as "" would send a customer an invoice email with a blank amount and report
/// success — silent-wrong, on the one path where wrong is expensive. The
/// notification dead-letters instead, with the field name in the reason.
pub fn interpolate(template: &str, doc: &serde_json::Value) -> Result<String, String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            return Err(format!("unclosed '{{{{' in template at byte {start}"));
        };
        let key = after[..end].trim();
        let value = doc
            .get(key)
            .ok_or_else(|| format!("template refers to unknown field '{key}'"))?;
        out.push_str(&render_field(value));
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

// ── recipients ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Recipient {
    /// Every active `app_user` with this role and an email address.
    Role(String),
    /// The address in this field of the document.
    Field(String),
    Literal(String),
}

impl Recipient {
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        let ident_ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if let Some(role) = spec.strip_prefix("role:") {
            return if ident_ok(role) {
                Ok(Recipient::Role(role.to_string()))
            } else {
                Err(format!("recipient role {role:?} must be a plain identifier"))
            };
        }
        if let Some(f) = spec.strip_prefix("field:") {
            return if ident_ok(f) {
                Ok(Recipient::Field(f.to_string()))
            } else {
                Err(format!("recipient field {f:?} must be a plain identifier"))
            };
        }
        if looks_like_address(spec) {
            Ok(Recipient::Literal(spec.to_string()))
        } else {
            Err(format!(
                "recipient {spec:?} is neither 'role:<role>', 'field:<fieldname>', nor an address"
            ))
        }
    }
}

/// Deliberately shallow: lettre's `Mailbox` parse is the real validator at send
/// time, and duplicating RFC 5322 here would be a second, worse implementation.
/// This only catches the "you meant role: and typo'd it" case at the door.
pub fn looks_like_address(s: &str) -> bool {
    match s.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && domain.contains('.') && !domain.starts_with('.')
                && !domain.ends_with('.') && !s.contains(char::is_whitespace)
        }
        None => false,
    }
}

/// Why a notification produced no message. Both variants are LOUD: they land in
/// the outbox as a dead row with this reason, and in `/metrics`.
#[derive(Debug, Clone, PartialEq)]
pub enum Unroutable {
    /// The rule matched but resolved to zero addresses.
    NoRecipients,
    /// Subject or body referred to a field the document does not have.
    Template(String),
}

impl std::fmt::Display for Unroutable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unroutable::NoRecipients => write!(
                f,
                "E_MAIL_NO_RECIPIENTS: the rule matched but resolved to zero addresses"
            ),
            Unroutable::Template(d) => write!(f, "E_MAIL_TEMPLATE: {d}"),
        }
    }
}

impl Unroutable {
    pub fn reason_label(&self) -> &'static str {
        match self {
            Unroutable::NoRecipients => "no_recipients",
            Unroutable::Template(_) => "template",
        }
    }
}

/// Render a matched rule into a concrete message.
///
/// `role_addresses` is supplied by the caller (it needs a database, and query
/// text does not live in this module) — so this function stays pure and
/// unit-testable, which is why every interpolation and recipient case below can
/// be proven without a server.
pub fn render(
    rule: &NotificationDef,
    doc: &serde_json::Value,
    role_addresses: &dyn Fn(&str) -> Vec<String>,
) -> Result<Outbound, Unroutable> {
    let mut to: Vec<String> = Vec::new();
    for spec in &rule.recipients {
        // parse() already passed validate() at write time; a stored rule that
        // somehow holds a bad spec is skipped rather than crashing the worker.
        let Ok(r) = Recipient::parse(spec) else { continue };
        match r {
            Recipient::Literal(a) => to.push(a),
            Recipient::Role(role) => to.extend(role_addresses(&role)),
            Recipient::Field(f) => {
                if let Some(v) = doc.get(&f) {
                    let a = render_field(v);
                    if looks_like_address(&a) {
                        to.push(a);
                    }
                }
            }
        }
    }
    to.sort();
    to.dedup();
    if to.is_empty() {
        return Err(Unroutable::NoRecipients);
    }
    let subject = interpolate(&rule.subject, doc).map_err(Unroutable::Template)?;
    let body = interpolate(&rule.body, doc).map_err(Unroutable::Template)?;
    Ok(Outbound { to, subject, body })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> serde_json::Value {
        serde_json::json!({
            "id": "sales_invoice:abc",
            "customer": "ACME Corp",
            // as SurrealDB returns a decimal: a string, trailing zeros and all
            "total": "20.00",
            "workflow_state": "Submitted for Approval",
            "contact": "ap@acme.example.com",
            "docstatus": 0,
        })
    }

    fn rule() -> NotificationDef {
        NotificationDef {
            name: "invoice_submitted".into(),
            doctype: "sales_invoice".into(),
            event: "on_transition".into(),
            action: Some("Submit".into()),
            condition: None,
            recipients: vec!["role:manager".into()],
            subject: "Invoice for {{customer}} needs approval".into(),
            body: "Total {{total}} is awaiting you.".into(),
            enabled: true,
        }
    }

    #[test]
    fn money_interpolates_verbatim_never_reformatted() {
        // The whole point of ADR-007 in a template: 20.00 stays 20.00. A
        // formatter that "helpfully" printed 20 or 20.0 would be arithmetic
        // nobody asked for, on the field where it matters most.
        let out = interpolate("Total {{total}}.", &doc()).unwrap();
        assert_eq!(out, "Total 20.00.");
    }

    #[test]
    fn unknown_field_is_an_error_not_an_empty_string() {
        let err = interpolate("Total {{totl}}.", &doc()).unwrap_err();
        assert!(err.contains("totl"), "the reason must name the field: {err}");
        // and the mistake reaches the caller as a dead-letter reason
        let mut r = rule();
        r.body = "Total {{totl}}".into();
        let e = render(&r, &doc(), &|_| vec!["m@x.example.com".into()]).unwrap_err();
        assert_eq!(e.reason_label(), "template");
    }

    #[test]
    fn event_and_action_both_gate_the_rule() {
        let r = rule();
        let d = doc();
        assert!(r.matches(&Trigger::Transition { action: "Submit", to_state: "Submitted for Approval" }, &d));
        // right event, wrong action
        assert!(!r.matches(&Trigger::Transition { action: "Approve", to_state: "Approved" }, &d));
        // right action name, wrong event class
        assert!(!r.matches(&Trigger::Update, &d));
        // disabled means disabled
        let mut off = rule();
        off.enabled = false;
        assert!(!off.matches(&Trigger::Transition { action: "Submit", to_state: "x" }, &d));
    }

    #[test]
    fn condition_compares_money_as_decimal_not_as_text() {
        let mut r = rule();
        // "20.00" > "100" lexically, but 20.00 < 100 numerically. A string
        // comparison here would fire an approval email on every small invoice.
        r.condition = Some(Rule { field: "total".into(), op: "ge".into(), value: Some("100".into()), message: None });
        assert!(!r.matches(&Trigger::Transition { action: "Submit", to_state: "x" }, &doc()));
        r.condition = Some(Rule { field: "total".into(), op: "ge".into(), value: Some("10".into()), message: None });
        assert!(r.matches(&Trigger::Transition { action: "Submit", to_state: "x" }, &doc()));
    }

    #[test]
    fn zero_recipients_is_unroutable_not_a_silent_success() {
        let e = render(&rule(), &doc(), &|_| Vec::new()).unwrap_err();
        assert_eq!(e, Unroutable::NoRecipients);
        assert_eq!(e.reason_label(), "no_recipients");
    }

    #[test]
    fn recipients_resolve_from_role_field_and_literal() {
        let mut r = rule();
        r.recipients = vec![
            "role:manager".into(),
            "field:contact".into(),
            "ops@frust.example.com".into(),
        ];
        let out = render(&r, &doc(), &|role| {
            assert_eq!(role, "manager");
            vec!["mgr@acme.example.com".into()]
        })
        .unwrap();
        assert_eq!(
            out.to,
            vec!["ap@acme.example.com", "mgr@acme.example.com", "ops@frust.example.com"]
        );
        assert_eq!(out.subject, "Invoice for ACME Corp needs approval");
    }

    #[test]
    fn validate_refuses_the_rules_that_would_never_fire() {
        let bad = NotificationDef {
            name: "x".into(),
            doctype: "sales_invoice".into(),
            event: "on_sumbit".into(), // typo
            action: Some("Submit".into()), // and not valid for a non-transition
            condition: None,
            recipients: vec!["role manager".into()], // missing colon
            subject: "".into(),
            body: "hi".into(),
            enabled: true,
        };
        let problems = bad.validate();
        assert_eq!(problems.len(), 4, "expected 4 problems, got {problems:#?}");
        assert!(rule().validate().is_empty(), "the good rule must pass");
    }

    #[test]
    fn unknown_transport_refuses_rather_than_falling_back() {
        // The env var is process-global; this test owns it briefly. Serialised
        // against the other env test by running them in one #[test].
        let restore = std::env::var("FRUST_MAIL").ok();
        std::env::set_var("FRUST_MAIL", "smpt"); // the classic typo
        let err = Mailer::from_env().unwrap_err();
        assert!(err.to_string().contains("must be 'file' or 'smtp'"), "{err}");
        match restore {
            Some(v) => std::env::set_var("FRUST_MAIL", v),
            None => std::env::remove_var("FRUST_MAIL"),
        }
    }
}
