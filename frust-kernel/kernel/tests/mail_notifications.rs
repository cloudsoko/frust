//! WO-043: email batteries, end to end through the broker.
//!
//! The browser/`frust serve` proof lives in `frust-e2e/mail.spec.mjs` (criterion
//! 5, tested-seam≠wired). This file proves the parts a live run cannot isolate:
//! that a notification added at runtime fires with no restart, that a rendered
//! message reaches the FILE transport with the right CONTENT, that a dead
//! transport bounds its retries and dead-letters, and — measured, not
//! asserted-by-architecture — that the save floor does not move when SMTP is
//! down.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::{Value, WriteOp};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::mail::{Mailer, MAX_ATTEMPTS, OUTBOX_TABLE};
use frust_kernel::tenancy::ResolvedTenant;
use frust_kernel::worker::MailWorker;

fn manager() -> Caller {
    Caller { user: "manager".into(), pass: "pw-manager".into(), role: "manager".into() }
}

fn from() -> lettre::message::Mailbox {
    "frust@test.example.com".parse().unwrap()
}

/// A scratch directory for one test's captured mail. Under `target/`, so a
/// `cargo clean` sweeps it and no test writes to a shared location.
fn maildir(name: &str) -> std::path::PathBuf {
    let d = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/mailtest")
        .join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("maildir");
    d
}

/// The kernel-owned mail tables, which `common::seeded_broker` does not build
/// (it seeds the WO-002 fixture, not the full meta schema).
fn install_mail_meta(db: &Db) {
    db.sql_root(&frust_kernel::meta::mail_ddl()).expect("mail meta ddl");
    // v5 added `email` to the SCHEMAFULL identity table
    db.sql_root("DEFINE FIELD OVERWRITE email ON app_user TYPE option<string>")
        .expect("app_user.email");
}

fn set_email(db: &Db, user: &str, email: &str) {
    db.sql_root(&format!("UPDATE app_user:{user} SET email = '{email}';")).expect("set email");
}

/// Add a notification rule the way the running kernel does: a record write plus
/// the generation bump. (`POST /notification` is the same two steps, and the
/// e2e drives that route.)
fn add_notification(db: &Db, json: serde_json::Value) {
    let name = json.get("name").and_then(|n| n.as_str()).expect("name").to_string();
    // validate through the same code path the REST route uses, so a broken
    // fixture fails here rather than silently never firing
    let rule: frust_kernel::mail::NotificationDef =
        serde_json::from_value(json.clone()).expect("notification shape");
    assert!(rule.validate().is_empty(), "fixture rule is invalid: {:?}", rule.validate());
    let content = frust_kernel::surql::render_value(&json_to_value(&json)).expect("render");
    db.sql_root(&format!("CREATE notification:{name} CONTENT {content};")).expect("create rule");
    frust_kernel::broker::invalidate_meta(db.tenant_id());
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

fn outbox(db: &Db) -> Vec<serde_json::Value> {
    db.sql_root(&format!("SELECT * FROM {OUTBOX_TABLE} ORDER BY enqueued_at;"))
        .expect("outbox")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn field<'a>(row: &'a serde_json::Value, k: &str) -> &'a str {
    row.get(k).and_then(|v| v.as_str()).unwrap_or_default()
}

fn create_order(broker: &Broker, title: &str, total: &str) -> serde_json::Value {
    broker
        .db_write(
            &manager(),
            &HookChain::default(),
            WriteOp::Create,
            "purchase_order",
            None,
            &[
                ("title".into(), Value::Text(title.into())),
                ("total".into(), Value::Decimal(total.into())),
            ],
        )
        .expect("create order")
}

fn fixture(name: &str) -> (Arc<Broker>, ResolvedTenant, Db) {
    let (broker, cfg) = common::seeded_broker(name);
    let db = scoped_db(&cfg);
    install_mail_meta(&db);
    (broker, cfg, db)
}

// ── criterion 1: metadata, no recompile, no restart ─────────────────────────

#[test]
fn a_notification_added_at_runtime_fires_on_the_very_next_write() {
    let (broker, _cfg, db) = fixture("mail_runtime");
    set_email(&db, "manager", "mgr@test.example.com");

    // A write BEFORE any rule exists — this also warms the (empty) rule cache,
    // which is the half that would break "no restart" if invalidation were
    // missing. Without this first write the test would pass on a cold cache and
    // prove nothing.
    create_order(&broker, "before the rule", "10.00");
    assert!(outbox(&db).is_empty(), "no rule yet, so nothing may be enqueued");

    add_notification(
        &db,
        serde_json::json!({
            "name": "po_created",
            "doctype": "purchase_order",
            "event": "after_insert",
            "recipients": ["role:manager"],
            "subject": "New PO: {{title}}",
            "body": "{{title}} for {{total}}.",
        }),
    );

    let stored = create_order(&broker, "after the rule", "42.50");
    let rows = outbox(&db);
    assert_eq!(rows.len(), 1, "the new rule must fire without a restart: {rows:#?}");
    let row = &rows[0];
    assert_eq!(field(row, "status"), "queued");
    assert_eq!(field(row, "notification"), "po_created");
    assert_eq!(
        row.get("to").and_then(|t| t.as_array()).map(|a| a.len()),
        Some(1),
        "role:manager must resolve to the manager's address: {row:#?}"
    );
    assert_eq!(field(row, "subject"), "New PO: after the rule");

    // ── money renders the STORED decimal, byte for byte ──
    //
    // Asserted against what the database actually returned rather than against
    // the literal "42.50" this test wrote: SurrealDB normalises decimal scale,
    // so hardcoding the input would either pass for the wrong reason or fail on
    // a formatting change that is not ours. This form fails only if something
    // starts REFORMATTING or COMPUTING the value on the way into the template.
    let as_stored = frust_kernel::mail::render_field(stored.get("total").expect("total"));
    assert_eq!(
        field(row, "body"),
        format!("after the rule for {as_stored}."),
        "the template must render the stored decimal verbatim (no arithmetic, ADR-007)"
    );
}

/// A database that predates meta v5 has no `notification` table, and SurrealDB
/// answers a SELECT against a missing table with an ERROR, not an empty set. If
/// that reached the write path it would log `lvl:error` on EVERY save — which is
/// how operators learn to ignore errors (the WO-033 discipline), and it would do
/// it on the busiest path there is.
#[test]
fn a_database_with_no_notification_table_is_quiet_not_noisy() {
    // deliberately NOT calling install_mail_meta: this is the pre-v5 shape
    let (broker, cfg) = common::seeded_broker("mail_prev5");
    let db = scoped_db(&cfg);

    let rules = frust_kernel::sync::load_notifications(&db, "purchase_order")
        .expect("a missing notification table must read as 'no rules', not as a failure");
    assert!(rules.is_empty());

    // and the write path is unaffected
    create_order(&broker, "pre-v5 write", "3.00");

    // the mapping must be surgical, not a blanket catch: a DIFFERENT bad query
    // against the same door still fails loudly
    let err = db.sql_root("SELECT * FROM notification WHERE").unwrap_err();
    assert!(
        !matches!(&err, frust_kernel::contract::BrokerError::Db { detail } if detail.is_empty()),
        "an actually-broken query must still error: {err:?}"
    );
}

// ── criterion 3: FileTransport captures the message, content asserted ───────

#[test]
fn the_file_transport_captures_the_rendered_message_on_disk() {
    let (broker, _cfg, db) = fixture("mail_file");
    set_email(&db, "manager", "mgr@test.example.com");
    add_notification(
        &db,
        serde_json::json!({
            "name": "po_filed",
            "doctype": "purchase_order",
            "event": "after_insert",
            "recipients": ["role:manager", "ops@test.example.com"],
            "subject": "Filed: {{title}}",
            "body": "Total {{total}} awaiting review.",
        }),
    );
    create_order(&broker, "Widget order", "99.95");

    let dir = maildir("file");
    let mailer = Mailer::file(&dir, from()).expect("file mailer");
    let worker = MailWorker { db: &db, mailer: &mailer, worker_id: "test".into() };
    assert_eq!(worker.drain().expect("drain"), 1, "one message must be delivered");

    // the outbox agrees it sent
    let rows = outbox(&db);
    assert_eq!(field(&rows[0], "status"), "sent", "{:#?}", rows[0]);

    // and the message is on disk, with the CONTENT — not merely a receipt
    let eml = std::fs::read_dir(&dir)
        .expect("read maildir")
        .filter_map(Result::ok)
        .find(|e| e.path().extension().is_some_and(|x| x == "eml"))
        .map(|e| std::fs::read_to_string(e.path()).expect("read eml"))
        .expect("an .eml must exist");
    assert!(eml.contains("Subject: Filed: Widget order"), "subject missing:\n{eml}");
    assert!(eml.contains("mgr@test.example.com"), "role recipient missing:\n{eml}");
    assert!(eml.contains("ops@test.example.com"), "literal recipient missing:\n{eml}");
    assert!(eml.contains("From: frust@test.example.com"), "sender missing:\n{eml}");
    assert!(eml.contains("awaiting review"), "body missing:\n{eml}");

    // the envelope json is lettre's own — no second file transport needed
    let envelope = std::fs::read_dir(&dir)
        .expect("read maildir")
        .filter_map(Result::ok)
        .find(|e| e.path().extension().is_some_and(|x| x == "json"))
        .map(|e| std::fs::read_to_string(e.path()).expect("read json"))
        .expect("an envelope .json must exist");
    assert!(envelope.contains("mgr@test.example.com"), "envelope recipients:\n{envelope}");
}

// ── criterion 4: failure is bounded, observable, and never silent ───────────

#[test]
fn a_dead_transport_bounds_its_retries_then_dead_letters() {
    let (broker, _cfg, db) = fixture("mail_dead");
    set_email(&db, "manager", "mgr@test.example.com");
    add_notification(
        &db,
        serde_json::json!({
            "name": "po_dead",
            "doctype": "purchase_order",
            "event": "after_insert",
            "recipients": ["role:manager"],
            "subject": "Never arrives: {{title}}",
            "body": "-",
        }),
    );
    create_order(&broker, "doomed", "1.00");

    // port 9 is the discard port — nothing listens, so this is connection
    // refused rather than a slow timeout, which keeps the test fast while
    // exercising the identical transient path
    let mailer = Mailer::smtp("127.0.0.1:9", from()).expect("smtp mailer");
    let worker = MailWorker { db: &db, mailer: &mailer, worker_id: "test".into() };

    for attempt in 1..MAX_ATTEMPTS {
        assert_eq!(worker.drain().expect("drain"), 0, "a dead relay delivers nothing");
        let rows = outbox(&db);
        assert_eq!(
            field(&rows[0], "status"),
            "queued",
            "attempt {attempt} must return the message to the queue, not drop it: {:#?}",
            rows[0]
        );
        assert_eq!(
            rows[0].get("attempts").and_then(|a| a.as_i64()),
            Some(attempt),
            "the attempt counter is what BOUNDS the retry — it must advance"
        );
    }

    // the last attempt exhausts the bound
    assert_eq!(worker.drain().expect("drain"), 0);
    let rows = outbox(&db);
    assert_eq!(field(&rows[0], "status"), "dead", "{:#?}", rows[0]);
    assert_eq!(rows[0].get("attempts").and_then(|a| a.as_i64()), Some(MAX_ATTEMPTS));
    assert!(!field(&rows[0], "last_error").is_empty(), "a dead letter must say why");

    // and it does not loop: a dead message is never claimed again
    assert_eq!(worker.drain().expect("drain"), 0);
    assert_eq!(
        outbox(&db)[0].get("attempts").and_then(|a| a.as_i64()),
        Some(MAX_ATTEMPTS),
        "a dead-lettered message must not keep being retried"
    );

    // typed in /metrics, per criterion 4 — a failure nobody can see is silent
    let metrics = frust_kernel::telemetry::render_prometheus();
    assert!(
        metrics.contains("frust_mail_dead_total"),
        "the dead-letter must be exported: {metrics}"
    );
    assert!(metrics.contains("frust_mail_retry_total"), "the retries must be exported");
}

#[test]
fn a_rule_that_resolves_to_nobody_dead_letters_instead_of_sending_nothing() {
    let (broker, _cfg, db) = fixture("mail_nobody");
    // deliberately NO email on any user with this role
    add_notification(
        &db,
        serde_json::json!({
            "name": "po_nobody",
            "doctype": "purchase_order",
            "event": "after_insert",
            "recipients": ["role:auditor"],
            "subject": "To nobody",
            "body": "-",
        }),
    );
    create_order(&broker, "unaddressed", "5.00");

    let rows = outbox(&db);
    assert_eq!(rows.len(), 1, "an unroutable notification must still leave a record");
    assert_eq!(field(&rows[0], "status"), "dead");
    assert!(
        field(&rows[0], "last_error").contains("E_MAIL_NO_RECIPIENTS"),
        "the reason must be the typed code, not prose: {:#?}",
        rows[0]
    );
}

// ── criterion 2: the save floor, MEASURED ───────────────────────────────────

/// **Criterion 2, as the WO states it:** "compare save latency with the mail
/// worker healthy vs. with a dead/slow transport — the save floor must not
/// move". So that is the comparison: the same workload, the same rule, a real
/// worker thread draining concurrently, and the only variable is whether the
/// transport works.
///
/// A third arm (no rule at all) is measured too, because it turned up a real
/// number worth reporting rather than hiding — see the diagnosis printed below.
///
/// Vacuity guards, both explicit: the rule must actually have fired (or the two
/// arms are the same workload and pass for the wrong reason), and the dead
/// transport must actually be slow (or "dead" is indistinguishable from
/// "healthy" and the comparison proves nothing).
#[test]
fn the_save_floor_does_not_move_when_smtp_is_dead() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc as StdArc;

    const N: usize = 20;

    let (broker, cfg, db) = fixture("mail_floor");
    set_email(&db, "manager", "mgr@test.example.com");

    let median = |mut v: Vec<Duration>| {
        v.sort();
        v[v.len() / 2]
    };
    let run = |tag: &str| {
        let mut d = Vec::with_capacity(N);
        for i in 0..N {
            let t = Instant::now();
            create_order(&broker, &format!("{tag} {i}"), "1.00");
            d.push(t.elapsed());
        }
        d
    };

    // ── arm A: no rule at all — the plain save floor ──
    let plain = median(run("plain"));
    assert!(outbox(&db).is_empty(), "with no rule, nothing may be enqueued");

    add_notification(
        &db,
        serde_json::json!({
            "name": "po_floor",
            "doctype": "purchase_order",
            "event": "after_insert",
            "recipients": ["role:manager"],
            "subject": "Floor {{title}}",
            "body": "{{total}}",
        }),
    );

    // A real worker thread, exactly as `frust serve` runs it: its own thread,
    // draining while saves happen. Measuring against an idle queue would prove
    // nothing about contention between the two.
    let spawn_worker = |relay: Option<&str>, dir: Option<std::path::PathBuf>| {
        let stop = StdArc::new(AtomicBool::new(false));
        let attempts = StdArc::new(AtomicUsize::new(0));
        let cfg = cfg.clone();
        let relay = relay.map(str::to_string);
        let (s2, a2) = (StdArc::clone(&stop), StdArc::clone(&attempts));
        let handle = std::thread::spawn(move || {
            let mailer = match (relay, dir) {
                (Some(r), _) => Mailer::smtp(r, from()).expect("smtp mailer"),
                (None, Some(d)) => Mailer::file(d, from()).expect("file mailer"),
                _ => unreachable!(),
            };
            let db = scoped_db(&cfg);
            while !s2.load(Ordering::Relaxed) {
                let w = MailWorker { db: &db, mailer: &mailer, worker_id: "floor".into() };
                if let Ok(n) = w.drain() {
                    a2.fetch_add(n, Ordering::Relaxed);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        (stop, attempts, handle)
    };

    // ── arm B: rule attached, transport HEALTHY (file) ──
    let dir = maildir("floor");
    let (stop, delivered, handle) = spawn_worker(None, Some(dir.clone()));
    let healthy = median(run("healthy"));
    std::thread::sleep(Duration::from_millis(300)); // let the worker catch up
    stop.store(true, Ordering::Relaxed);
    handle.join().expect("worker thread");
    let delivered = delivered.load(Ordering::Relaxed);

    // ── arm C: rule attached, transport a BLACK HOLE (unroutable address) ──
    // Every send now blocks for the full 10 s timeout on the worker thread. If
    // any of that leaked onto the request path, this median would explode.
    let (stop, _a, handle) = spawn_worker(Some("10.0.0.0:2525"), None);
    let dead = median(run("dead"));
    stop.store(true, Ordering::Relaxed);

    // ── vacuity guards ──
    let rows = outbox(&db);
    assert_eq!(rows.len(), 2 * N, "each save under a rule must enqueue exactly one message");
    assert!(delivered >= 1, "the healthy arm must actually have delivered mail, or 'healthy' is a label not a fact");
    assert!(
        std::fs::read_dir(&dir).expect("maildir").filter_map(Result::ok).count() >= 2,
        "the healthy worker must have written captured mail to disk"
    );

    // and the black hole is genuinely slow — one send, measured
    let mailer = Mailer::smtp("10.0.0.0:2525", from()).expect("smtp mailer");
    let t = Instant::now();
    let attempt = mailer.deliver(&frust_kernel::mail::Outbound {
        to: vec!["mgr@test.example.com".into()],
        subject: "black hole".into(),
        body: "-".into(),
    });
    let one_send = t.elapsed();
    assert!(attempt.is_err(), "the black-hole transport must deliver nothing");
    assert!(
        one_send > Duration::from_secs(1),
        "the dead arm is meaningless unless a send against this address really blocks (took {one_send:?})"
    );
    handle.join().expect("worker thread");

    println!(
        "SAVE FLOOR — no rule {plain:?} | rule + HEALTHY transport {healthy:?} | \
         rule + DEAD transport {dead:?} | one blocked SMTP send {one_send:?}"
    );

    // ── the WO's criterion: healthy vs dead must be the same floor ──
    let (lo, hi) = if healthy < dead { (healthy, dead) } else { (dead, healthy) };
    assert!(
        hi < lo * 2 && hi < lo + Duration::from_millis(20),
        "the save floor MOVED when SMTP died (healthy {healthy:?} vs dead {dead:?}) — \
         the request path is paying transport latency"
    );
    assert!(
        dead * 20 < one_send,
        "a save ({dead:?}) must be orders of magnitude cheaper than the SMTP wait the \
         worker absorbed for it ({one_send:?})"
    );

    // ── the honest extra number: attaching a rule DOES cost a save something ──
    //
    // Not SMTP latency — a durable enqueue, which is one `sql_root` INSERT. That
    // A rule-attached save costs exactly two extra root round trips: the
    // `role:` recipient lookup (a read) and the outbox row (a write). So the
    // yardstick is those two operations, MEASURED here — not a constant, and
    // not a read used to stand in for a write.
    //
    // WO-044 is why this is measured as two separate operations. Before it, root
    // Basic auth cost ~16 ms per request (SurrealDB argon2-verifies the password
    // on every call), which swamped the difference between a read and a write
    // and made "4 × a trivial SELECT" a fine approximation. With the root JWT
    // the auth cost is ~450 µs and the WRITE is now the dominant term, so a
    // read-based yardstick understates the floor by ~5×. The optimisation
    // changed which thing this test has to measure.
    let probe = |q: &str| {
        let mut v = Vec::new();
        for i in 0..10 {
            let t = Instant::now();
            db.sql_root(&q.replace("{i}", &i.to_string())).expect("probe");
            v.push(t.elapsed());
        }
        median(v)
    };
    let read_rtt = probe("SELECT id FROM app_user LIMIT 1;");
    let write_rtt = probe("CREATE _frust_mail_probe CONTENT { i: {i} };");
    let budget = read_rtt + write_rtt;
    let overhead = healthy.saturating_sub(plain);
    println!(
        "  note: rule-attached saves cost {overhead:?} more than plain — two root round trips          (read {read_rtt:?} + write {write_rtt:?} = {budget:?}), not SMTP."
    );
    assert!(
        overhead < budget * 3,
        "a rule-attached save should cost about one root read plus one root write          ({budget:?}), not {overhead:?} — if this fails, the notification path grew a query"
    );
}
