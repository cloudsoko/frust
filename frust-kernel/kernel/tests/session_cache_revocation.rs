//! The session cache sits in a PERMISSION path, so revocation is the gate,
//! not the throughput.
//!
//! Caching the token→principal lookup removes the last per-request DB round
//! trip. It also creates the possibility of serving a revoked principal, which
//! would be a silent-wrong defect reborn in the worst possible place. These
//! tests prove logout is IMMEDIATE (generation bump) and that a cache hit is
//! behaviourally identical to a miss.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::Broker;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::meta_ddl;
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

const ADDR: &str = "127.0.0.1:8796";

fn boot() -> &'static str {
    static K: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        let cfg = single_tenant(&"sess_cache".to_string()).expect("tenancy");
        let db = scoped_db(&cfg);
        db.sql_root_ns("REMOVE DATABASE IF EXISTS sess_cache; DEFINE DATABASE sess_cache;").unwrap();
        db.sql_root(&meta_ddl()).unwrap();
        db.sql_root(
            "CREATE app_user:u SET name = 'u', role = 'manager', pass = crypto::argon2::generate('pw-u'); \
             CREATE doctype CONTENT { name: 'sc_doc', fields: [ { fieldname: 'title', fieldtype: 'Data' } ] };",
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

fn login() -> String {
    let (_, v) = post("/login", None, r#"{"user":"u","pass":"pw-u"}"#);
    v["token"].as_str().unwrap_or_default().to_string()
}

/// **THE GATE: logout is immediate.** The token is used repeatedly first, so it
/// is certainly cached; the very next request after logout must be refused.
#[test]
fn logout_invalidates_the_cached_session_immediately() {
    let tok = login();

    // warm the cache hard — several successful authenticated requests
    for _ in 0..5 {
        let (code, _) = post("/read/sc_doc", Some(&tok), "{}");
        assert_eq!(code, 200, "authenticated read before logout");
    }

    let (code, _) = post("/logout", Some(&tok), "{}");
    assert_eq!(code, 200, "logout");

    // the NEXT request — no sleep, no TTL wait — must be 401
    let (code, out) = post("/read/sc_doc", Some(&tok), "{}");
    assert_eq!(
        code, 401,
        "a revoked token must NOT be served from cache for even one request: {out}"
    );
}

/// One session's logout must not silently authorise another user's token
/// either — the coarse generation bump drops all entries, and every one of them
/// must then re-read rather than fail.
#[test]
fn other_sessions_survive_a_logout_by_re_reading() {
    let a = login();
    let b = login();
    assert_eq!(post("/read/sc_doc", Some(&a), "{}").0, 200);
    assert_eq!(post("/read/sc_doc", Some(&b), "{}").0, 200);

    assert_eq!(post("/logout", Some(&a), "{}").0, 200, "logout a");

    // a is revoked; b was only invalidated from cache and must still work
    assert_eq!(post("/read/sc_doc", Some(&a), "{}").0, 401, "a is revoked");
    assert_eq!(
        post("/read/sc_doc", Some(&b), "{}").0,
        200,
        "b must re-read from the session table, not be collateral damage"
    );
}

/// A cache HIT must be behaviourally identical to a MISS — same principal,
/// same role, same row visibility. If the hit path forgot to re-seed the JWT,
/// the row half would break on the second request.
#[test]
fn a_cache_hit_behaves_exactly_like_a_miss() {
    let tok = login();
    let (c1, first) = post("/read/sc_doc", Some(&tok), "{}"); // miss
    let (c2, second) = post("/read/sc_doc", Some(&tok), "{}"); // hit
    let (c3, third) = post("/read/sc_doc", Some(&tok), "{}"); // hit
    assert_eq!((c1, c2, c3), (200, 200, 200));
    assert_eq!(first, second, "hit must equal miss");
    assert_eq!(second, third, "and stay equal");
}

/// Garbage and empty tokens are still refused with the cache in place — the
/// cache must not become an authentication bypass for unknown tokens.
#[test]
fn unknown_tokens_are_still_refused() {
    let _ = login();
    assert_eq!(post("/read/sc_doc", Some("notarealtoken123"), "{}").0, 401);
    assert_eq!(post("/read/sc_doc", None, "{}").0, 401);
}
