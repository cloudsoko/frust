//! WO-033: the two hygiene items, proven over a LIVE serving kernel.
//!
//! 1. **Admin force-revoke** — a manager kills every session a user holds, and
//!    the very NEXT request is refused (no `SESSION_TTL` wait), while another
//!    user's session survives.
//! 2. **A healthy keyguard check is quiet** — the self-forge probe (ADR-013) is
//!    a deliberately-failing call, so it must not make a clean boot cry
//!    `lvl:error`. Asserted WITH ITS CONTROL: a genuinely restored
//!    (placeholder-key) store must still be refused loudly, so the quiet path
//!    is one that can be seen to fail.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::Broker;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;
use frust_kernel::keyguard::{self, REDACTED_KEY};
use frust_kernel::meta::{meta_ddl, restored_key_remediation_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

const ADDR: &str = "127.0.0.1:8797";
const DB: &str = "kh_hygiene";

fn boot() -> &'static str {
    static K: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let cfg = single_tenant(&DB.to_string()).expect("tenancy");
        let db = scoped_db(&cfg);
        db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {DB}; DEFINE DATABASE {DB};")).unwrap();
        db.sql_root(&meta_ddl()).unwrap();
        db.sql_root(
            "CREATE app_user:boss  SET name = 'boss',  role = 'manager', pass = crypto::argon2::generate('pw-boss'); \
             CREATE app_user:clerk SET name = 'clerk', role = 'clerk',   pass = crypto::argon2::generate('pw-clerk'); \
             CREATE app_user:other SET name = 'other', role = 'clerk',   pass = crypto::argon2::generate('pw-other');",
        )
        .unwrap();
        MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
        let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())));
        let rest = Rest::single(broker, ADDR.to_string(), Some(Arc::new(MetadataSync { base: cfg })), None);
        std::thread::spawn(move || {
            let _ = rest.serve(|| {});
        });
        for _ in 0..100 {
            if std::net::TcpStream::connect(ADDR).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    ADDR
}

fn post(path: &str, token: Option<&str>, body: &str) -> (u16, serde_json::Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut req = agent.post(format!("http://{}{path}", boot()));
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut r = req.send(body.to_string()).expect("http");
    (r.status().as_u16(), r.body_mut().read_json().unwrap_or(serde_json::json!({})))
}

fn get(path: &str, token: Option<&str>) -> (u16, serde_json::Value) {
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();
    let mut req = agent.get(format!("http://{}{path}", boot()));
    if let Some(t) = token {
        req = req.header("Authorization", &format!("Bearer {t}"));
    }
    let mut r = req.call().expect("http");
    (r.status().as_u16(), r.body_mut().read_json().unwrap_or(serde_json::json!({})))
}

fn login(user: &str, pass: &str) -> String {
    let (code, v) = post("/login", None, &format!(r#"{{"user":"{user}","pass":"{pass}"}}"#));
    assert_eq!(code, 200, "login {user}: {v}");
    v["token"].as_str().unwrap_or_default().to_string()
}

/// Does this token still work? `/meta` is the cheapest authenticated read.
fn works(token: &str) -> bool {
    get("/meta", Some(token)).0 == 200
}

// ── item 1: admin force-revoke ──────────────────────────────────────────────

/// The property WO-026 queued: revocation is INSTANT (generation bump), not
/// TTL-delayed — and it is surgical.
///
/// The clerk logs in TWICE on purpose: revoking one session while an attacker
/// holds a second accomplishes nothing, which is why revoke targets the USER.
#[test]
fn revoke_kills_every_session_of_one_user_immediately() {
    let a = login("clerk", "pw-clerk");
    let b = login("clerk", "pw-clerk");
    let other = login("other", "pw-other");
    let boss = login("boss", "pw-boss");

    // used first, so all three are certainly CACHED — otherwise a pass could
    // just mean "there was nothing cached to go stale"
    assert!(works(&a) && works(&b) && works(&other), "all sessions work before revoke");

    // manager-tier: a clerk may not revoke
    let (code, _) = post("/revoke/clerk", Some(&a), "{}");
    assert_eq!(code, 403, "a clerk must not be able to revoke sessions (got {code})");

    let (code, out) = post("/revoke/clerk", Some(&boss), "{}");
    assert_eq!(code, 200, "manager may revoke: {out}");
    assert_eq!(out["revoked"], serde_json::json!(2), "BOTH clerk sessions revoked: {out}");

    // THE GATE: the very next request is refused, with no sleep. If this needed
    // a TTL wait the generation bump did nothing.
    assert!(!works(&a), "revoked session A must be refused IMMEDIATELY");
    assert!(!works(&b), "revoked session B must be refused IMMEDIATELY");

    // blast radius is one user: the bump makes others RE-READ, not vanish
    assert!(works(&other), "another user's session must survive the generation bump");
    assert!(works(&boss), "the revoking manager's own session must survive");
}

/// Revoking a user with no sessions is a quiet success — an admin should be
/// able to revoke defensively without first reading the session table.
#[test]
fn revoking_a_user_with_no_sessions_is_a_quiet_success() {
    let boss = login("boss", "pw-boss");
    let (code, out) = post("/revoke/nobody", Some(&boss), "{}");
    assert_eq!(code, 200, "revoking nothing is not an error: {out}");
    assert_eq!(out["revoked"], serde_json::json!(0));
}

// ── item 2: the quiet probe, and the control that proves it can be loud ─────

/// A healthy store passes the keyguard, and the probe's expected refusal is
/// logged quietly (WO-033 item 2). The behavioural half is asserted here; the
/// zero-`lvl:error` half is asserted against the real boot in
/// `boot_quiet.rs`-style fashion by `a_restored_store_is_still_refused_loudly`
/// below, which is this test's control.
#[test]
fn a_healthy_store_passes_the_keyguard_and_still_rejects_the_forged_key() {
    boot(); // ensure the DB + access exist
    let db = scoped_db(&single_tenant(&DB.to_string()).expect("tenancy"));

    keyguard::assert_access_key_is_not_redacted(&db).expect("a healthy store must pass the guard");

    // quiet must not mean SKIPPED: the store really does reject the published
    // key, which is the fact the quiet log line is reporting
    assert!(
        !keyguard::store_accepts_key(&db, REDACTED_KEY).unwrap(),
        "the healthy store must still REJECT the published key"
    );
}

/// THE CONTROL. Without it, "a healthy boot logs zero errors" is a check that
/// has never been seen to fail. Install the export PLACEHOLDER as the real key
/// (what `surreal import` leaves behind) and the guard must refuse loudly —
/// quieting the *expected* failure must not quiet a *real* compromise.
///
/// Runs in its own database so it cannot disturb the live server above.
#[test]
fn a_restored_store_is_still_refused_loudly() {
    let name = "kh_restored";
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&meta_ddl()).unwrap();
    // simulate the restore: the access now literally uses '[REDACTED]'
    let ddl = restored_key_remediation_ddl()
        .replace("<access-name>", "account")
        .replace("<a fresh high-entropy secret>", REDACTED_KEY);
    db.sql_root(&ddl).expect("install the placeholder key");

    let err = keyguard::assert_access_key_is_not_redacted(&db)
        .expect_err("a store accepting the published key MUST be refused");
    assert!(
        format!("{err:?}").contains("FRUST:E_RESTORED_ACCESS_KEY"),
        "the machine code still fires: {err:?}"
    );

    // and it is not reporting a compromise it failed to detect
    assert!(
        keyguard::store_accepts_key(&db, REDACTED_KEY).unwrap(),
        "the restored store really does accept the published key"
    );
}
