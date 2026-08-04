//! **One tenant's logout must not cold-start another's cache.**
//!
//! The kernel avoids a per-request DB round trip by caching the
//! token→principal lookup, invalidated by generation. The generation was
//! process-global, so with N tenants in one process "invalidate on logout"
//! meant *everyone's* cache — never wrong, but a noisy-neighbour vector aimed
//! at the cache that took throughput from 15 to 124 req/s. Keying the
//! generation per tenant closes it, and this is the proof.
//!
//! **How the survival is observed, and why it cannot pass by accident.**
//! Counting cache hits would prove nothing an off-by-one couldn't fake. So
//! the instrument is the session row itself: B's row is deleted out of band
//! *before* A logs out. From then on B's token can only resolve **from
//! cache** — a miss goes to the database and finds nothing. So:
//!
//! - B still works after A's logout  ⇒  B was served from cache, i.e. A's
//!   logout did not touch it.
//! - and the control below bumps B's *own* generation and shows B then fails,
//!   which is what stops "B still works" from meaning "invalidation is broken
//!   everywhere".
//!
//! Requires the dev surreal on :8899.

use std::sync::Arc;

use frust_kernel::broker::{Broker, HookDispatch};
use frust_kernel::contract::{BrokerError, Value};
use frust_kernel::db::scoped_db;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

struct PassHooks;
impl HookDispatch for PassHooks {
    fn validate(&self, _d: &str, doc: &[(String, Value)]) -> Result<Vec<(String, Value)>, BrokerError> {
        Ok(doc.to_vec())
    }
}

fn seed(tenant: &str) -> ResolvedTenant {
    let target = single_tenant(tenant).expect("tenancy");
    let db = scoped_db(&target);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {tenant}; DEFINE DATABASE {tenant};"))
        .expect("provision");
    db.sql_root(&format!(
        "{}; {}; DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE app_user:m SET name = 'm', role = 'manager', pass = crypto::argon2::generate('pw-m'); \
         CREATE doctype CONTENT {{ name: 'note', label: 'Note', fields: [ \
            {{ fieldname: 'title', fieldtype: 'Data' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .expect("seed");
    MetadataSync { base: target.clone() }.sync(&db).expect("sync");
    // a row that names its owner, so every read below is provenance-checkable
    db.sql_root(&format!(
        "CREATE note SET title = '{tenant}-note', owner = app_user:m, status = 'Draft';"
    ))
    .expect("seed row");
    target
}

fn start_rest(target: &ResolvedTenant, port: u16) -> String {
    let broker = Arc::new(Broker::new(scoped_db(target), Box::new(PassHooks)));
    let addr = format!("127.0.0.1:{port}");
    let url = format!("http://{addr}");
    std::thread::spawn(move || {
        let _ = Rest::single(broker, addr, None, None).serve(|| {});
    });
    for _ in 0..50 {
        if ureq::get(format!("{url}/health")).call().is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    url
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().http_status_as_error(false).build().into()
}

fn login(url: &str) -> String {
    let mut r = agent()
        .post(format!("{url}/login"))
        .send(serde_json::json!({ "user": "m", "pass": "pw-m" }).to_string())
        .expect("login");
    let body: serde_json::Value = r.body_mut().read_json().expect("login json");
    body["token"].as_str().expect("token").to_string()
}

/// Read `note` as the session behind `token`. Returns the status and the
/// titles, so callers assert **whose** rows came back, not merely that the
/// call worked.
fn read_notes(url: &str, token: &str) -> (u16, Vec<String>) {
    let mut r = agent()
        .post(format!("{url}/read/note"))
        .header("Authorization", &format!("Bearer {token}"))
        .send("{}")
        .expect("read");
    let code = r.status().as_u16();
    let body: serde_json::Value = r.body_mut().read_json().unwrap_or(serde_json::json!({}));
    let titles = body["rows"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .map(|x| x["title"].as_str().unwrap_or("<missing>").to_string())
                .collect()
        })
        .unwrap_or_default();
    (code, titles)
}

/// The secret half of a `<tenant>.<random>` token — what the session row
/// actually stores (the prefix is routing, and the row already lives in the
/// tenant it names).
fn secret_of(token: &str) -> &str {
    token.split_once('.').expect("a minted token carries its tenant").1
}

fn logout(url: &str, token: &str) -> u16 {
    agent()
        .post(format!("{url}/logout"))
        .header("Authorization", &format!("Bearer {token}"))
        .send("{}")
        .expect("logout")
        .status()
        .as_u16()
}

#[test]
fn one_tenants_logout_leaves_the_other_tenants_session_cache_intact() {
    let a = seed("wo040b_sess_a");
    let b = seed("wo040b_sess_b");
    let url_a = start_rest(&a, 8811);
    let url_b = start_rest(&b, 8812);

    let tok_a = login(&url_a);
    let tok_b = login(&url_b);

    // warm both caches, and check provenance while we are here
    let (code, titles) = read_notes(&url_a, &tok_a);
    assert_eq!(code, 200, "A's first read: {titles:?}");
    assert_eq!(titles, vec!["wo040b_sess_a-note"], "A read someone else's rows");
    let (code, titles) = read_notes(&url_b, &tok_b);
    assert_eq!(code, 200, "B's first read: {titles:?}");
    assert_eq!(titles, vec!["wo040b_sess_b-note"], "B read someone else's rows");

    // THE INSTRUMENT: delete B's session row out of band. From here, B's
    // token resolves only if it is served from cache.
    scoped_db(&b)
        .sql_root(&format!("DELETE _frust_session WHERE token = '{}';", secret_of(&tok_b)))
        .expect("delete B's session row");

    // tenant A logs out — under the old process-global generation this
    // invalidated every cached session in the process, B's included
    assert_eq!(logout(&url_a, &tok_a), 200);

    let (code, titles) = read_notes(&url_b, &tok_b);
    assert_eq!(
        code, 200,
        "B's session was dropped by A's logout — the generation is still shared"
    );
    assert_eq!(
        titles,
        vec!["wo040b_sess_b-note"],
        "B was served, but not B's data — routing, not caching, is broken"
    );

    // ...and A really was invalidated: its token must now be refused
    let (code, _) = read_notes(&url_a, &tok_a);
    assert_eq!(code, 401, "A's own logout did not invalidate A");
}

/// **The control.** The test above concludes "B survived" from B still
/// working. That only means something if B's cache entry *can* be dropped —
/// otherwise "B still works" would be true even with invalidation entirely
/// broken, which is the assert-the-operation failure wearing a
/// different hat.
#[test]
fn the_same_instrument_shows_b_failing_when_b_is_the_one_invalidated() {
    let b = seed("wo040b_ctl_b");
    let url = start_rest(&b, 8813);
    let tok = login(&url);

    let (code, titles) = read_notes(&url, &tok);
    assert_eq!(code, 200, "warm-up: {titles:?}");

    // same out-of-band deletion as the test above...
    scoped_db(&b)
        .sql_root(&format!("DELETE _frust_session WHERE token = '{}';", secret_of(&tok)))
        .expect("delete session row");
    // ...and this time the tenant whose generation moves is B's own
    frust_kernel::tenant_gen::invalidate_sessions(b.tenant_id().as_str());

    let (code, _) = read_notes(&url, &tok);
    assert_eq!(
        code, 401,
        "B kept working after ITS OWN invalidation — the instrument cannot detect a dropped \
         cache entry, so the survival test above proves nothing"
    );
}
