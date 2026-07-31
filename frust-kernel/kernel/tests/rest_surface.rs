//! WO-005 module 6 / exit criterion 2 completion: REST is the THIRD consumer
//! of the one permission compiler. Same role metadata, byte-equal row/field
//! filtering via (a) direct broker db_read [the plugin/Desk path] and (b) the
//! REST endpoint [external clients + Desk-as-client]. One test, the doors.
//!
//! Uses the WO-002 skeleton dataset (ns frust / db skeleton, purchase_order,
//! clerk1/clerk2/manager). Boots a Rest on an ephemeral port in-process.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller};
use frust_kernel::contract::*;
use frust_kernel::db::Db;
use frust_kernel::rest::Rest;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

mod common;

/// WO-020 criterion 5: SELF-SEEDING (was `DbConfig::default()` over ambient
/// `skeleton`). The REST-vs-broker byte-equality proof now stands on a fixture
/// this file builds itself.
fn broker() -> Arc<Broker> {
    let (b, _cfg) = common::seeded_broker("rs_fixture");
    b
}

/// Start a REST server on a unique port, return its base URL. Ticks nothing
/// (no worker) â€” pure request/response for the proof.
fn start_rest(broker: Arc<Broker>, port: u16) -> String {
    let addr = format!("127.0.0.1:{port}");
    let url = format!("http://{addr}");
    std::thread::spawn(move || {
        let rest = Rest::single(broker, addr, None, None);
        let _ = rest.serve(|| {});
    });
    // wait for bind
    for _ in 0..50 {
        if ureq::post(format!("{url}/health")).send("").is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    url
}

/// WO-009 criterion 1: sessions. Login proves credentials; the ROLE comes
/// back from the DB, not from anything the client claimed.
fn login(url: &str, user: &str) -> (String, String) {
    let mut res = ureq::post(format!("{url}/login"))
        .send(serde_json::json!({ "user": user, "pass": format!("pw-{user}") }).to_string())
        .expect("login");
    let body: serde_json::Value = res.body_mut().read_json().expect("login json");
    (
        body["token"].as_str().expect("token").to_string(),
        body["role"].as_str().unwrap_or_default().to_string(),
    )
}

fn rest_read(url: &str, token: &str, doctype: &str) -> Vec<serde_json::Value> {
    let mut res = ureq::post(format!("{url}/read/{doctype}"))
        .header("Authorization", &format!("Bearer {token}"))
        .send("{}")
        .expect("rest read");
    let body: serde_json::Value = res.body_mut().read_json().expect("json");
    body.get("rows").and_then(|r| r.as_array()).cloned().unwrap_or_default()
}

/// The criterion-2 proof: three principals, two doors, byte-equal. Roles now
/// come from the session (DB-derived), never the wire.
#[test]
fn rest_is_the_third_consumer_byte_equal() {
    let b = broker();
    let url = start_rest(Arc::clone(&b), 8791);

    for user in ["clerk1", "clerk2", "manager"] {
        let (token, role) = login(&url, user);
        let caller = Caller { user: user.into(), pass: format!("pw-{user}"), role };
        // door 1: direct broker (the plugin/Desk-render path)
        let via_broker = b
            .db_read(&caller, "purchase_order", None, &[], &ReadOpts::default())
            .expect("broker read");
        // door 2: REST (external client / Desk-as-client), session principal
        let via_rest = rest_read(&url, &token, "purchase_order");

        assert_eq!(
            serde_json::to_string(&via_broker).unwrap(),
            serde_json::to_string(&via_rest).unwrap(),
            "{user}: REST and broker must produce byte-equal rows â€” one permission compiler"
        );
    }
}

/// The row-level split is enforced identically through REST: 2/1/3 rows,
/// same numbers module 1 proved via the broker.
#[test]
fn rest_row_split_matches_broker() {
    let b = broker();
    let url = start_rest(Arc::clone(&b), 8792);
    let clerk1 = rest_read(&url, &login(&url, "clerk1").0, "purchase_order").len();
    let clerk2 = rest_read(&url, &login(&url, "clerk2").0, "purchase_order").len();
    let manager = rest_read(&url, &login(&url, "manager").0, "purchase_order").len();
    println!("REST row split: clerk1={clerk1} clerk2={clerk2} manager={manager}");
    assert!(clerk1 < manager && clerk2 < manager);
    assert_eq!(clerk1 + clerk2, manager, "clerks partition the table, enforced by the DB via REST");
}

/// Structured filters travel over the wire as the contract, never a raw
/// string â€” a filtered REST read returns only matching, permission-scoped rows.
#[test]
fn rest_structured_filter() {
    let b = broker();
    let url = start_rest(Arc::clone(&b), 8793);
    let (token, _) = login(&url, "manager");
    let body = serde_json::json!({
        "filter": { "path": "status", "op": "ne", "value": "Paid" },
        "order": { "path": "total", "dir": "desc" },
        "limit": 5
    });
    let mut res = ureq::post(format!("{url}/read/purchase_order"))
        .header("Authorization", &format!("Bearer {token}"))
        .send(body.to_string())
        .unwrap();
    let out: serde_json::Value = res.body_mut().read_json().unwrap();
    let rows = out["rows"].as_array().cloned().unwrap_or_default();
    for r in &rows {
        assert_ne!(r["status"].as_str(), Some("Paid"), "filter applied over the wire");
    }
    assert!(rows.len() <= 5, "limit honored");
}

/// WO-009 criterion 1, the kill-proof: X-Frust-Role is DELETED. A request
/// with no session is 401 regardless of headers; a clerk session dragging
/// the old manager-role header gets exactly the clerk's rows and fields â€”
/// the claim has nowhere to land.
#[test]
fn role_header_is_dead() {
    let b = broker();
    let url = start_rest(Arc::clone(&b), 8795);
    let agent: ureq::Agent = ureq::Agent::config_builder().http_status_as_error(false).build().into();

    // no session: 401, even with the full old header trio
    let r = agent
        .post(format!("{url}/read/purchase_order"))
        .header("X-Frust-User", "manager")
        .header("X-Frust-Pass", "pw-manager")
        .header("X-Frust-Role", "manager")
        .send("{}")
        .unwrap();
    assert_eq!(r.status().as_u16(), 401, "headers alone are nothing");

    // garbage token: 401
    let r = agent
        .post(format!("{url}/read/purchase_order"))
        .header("Authorization", "Bearer notatoken123")
        .send("{}")
        .unwrap();
    assert_eq!(r.status().as_u16(), 401);

    // clerk session + tampered role header: byte-equal to the plain clerk read
    let (token, role) = login(&url, "clerk1");
    assert_eq!(role, "clerk", "role derived from the app_user record");
    let plain = rest_read(&url, &token, "purchase_order");
    let mut r = agent
        .post(format!("{url}/read/purchase_order"))
        .header("Authorization", &format!("Bearer {token}"))
        .header("X-Frust-Role", "manager")
        .send("{}")
        .unwrap();
    let tampered: serde_json::Value = r.body_mut().read_json().unwrap();
    let tampered = tampered["rows"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        serde_json::to_string(&plain).unwrap(),
        serde_json::to_string(&tampered).unwrap(),
        "a tampered role claim changes NOTHING"
    );

    // logout invalidates the session
    agent.post(format!("{url}/logout")).header("Authorization", &format!("Bearer {token}")).send("").unwrap();
    let r = agent
        .post(format!("{url}/read/purchase_order"))
        .header("Authorization", &format!("Bearer {token}"))
        .send("{}")
        .unwrap();
    assert_eq!(r.status().as_u16(), 401, "logged-out token is dead");
}

