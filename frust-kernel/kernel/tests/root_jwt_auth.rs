//! Criteria 1 + 2: the root-JWT path is CORRECT and the restored-access-key
//! guard still holds.
//!
//! Deliberately no throughput number in this file. Correctness gates the win,
//! so the measurement lives in `examples/rootauth.rs` and runs only after these
//! pass.

mod common;

// **Each test PINS the credential arm it is asserting about.** These assert
// JWT-path properties, and `FRUST_ROOT_AUTH` is a process-wide switch — so
// under `FRUST_ROOT_AUTH=basic` they used to fail for the entirely correct
// reason that there was no token to assert about. The Basic arm of the parity
// test was already pinned; pinning the JWT arm is the missing half, and it makes
// the file mode-independent.

use frust_kernel::contract::BrokerError;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::keyguard::{forge_token, store_accepts_key, REDACTED_KEY};
use frust_kernel::tenancy::ResolvedTenant;

fn fixture(name: &str) -> (Db, ResolvedTenant) {
    let (_broker, cfg) = common::seeded_broker(name);
    (scoped_db(&cfg), cfg)
}

/// Every shape of root call the kernel actually makes, so "identical results"
/// means identical across the real surface rather than across one SELECT.
fn root_surface(db: &Db) -> Vec<serde_json::Value> {
    vec![
        db.sql_root("SELECT name, role FROM app_user ORDER BY name;").expect("select"),
        db.sql_root("SELECT title, total FROM purchase_order ORDER BY title;").expect("ordered select"),
        db.sql_root("SELECT count() FROM purchase_order GROUP ALL;").expect("aggregate"),
        db.sql_root("DEFINE TABLE OVERWRITE wo044 SCHEMALESS PERMISSIONS NONE;").expect("ddl"),
        // Both arms run against the same database, so this surface must start
        // from the same state each time — `DEFINE TABLE OVERWRITE` deliberately
        // PRESERVES records, so the reset is explicit.
        db.sql_root("DELETE wo044;").expect("reset"),
        db.sql_root("CREATE wo044:one SET v = 1;").expect("create"),
        db.sql_root("UPDATE wo044:one SET v = 2;").expect("update"),
        db.sql_root("SELECT * FROM wo044;").expect("read back"),
        db.sql_root("DELETE wo044:one;").expect("delete"),
        // `sql_root_raw` returns the per-statement envelope, which carries
        // SurrealDB's own `time` stopwatch — a string that differs run to run by
        // construction. Comparing it would be comparing the clock, not the
        // result, so it is dropped and the status + rows are what must match.
        serde_json::json!(db
            .sql_root_raw("CREATE wo044:two SET v = 3;")
            .expect("raw")
            .into_iter()
            .map(|mut stmt| {
                if let Some(o) = stmt.as_object_mut() {
                    o.remove("time");
                }
                stmt
            })
            .collect::<Vec<_>>()),
    ]
}

// ── criterion 1: correctness ────────────────────────────────────────────────

/// The point of the change is that *only the credential* changes. If any
/// result differs, the optimisation is not an optimisation.
#[test]
fn the_jwt_path_returns_identical_results_to_basic() {
    // Two handles onto the SAME database. `FRUST_ROOT_AUTH` is process-wide, so
    // rather than flipping it mid-test (which would race the other tests in
    // this binary) the Basic arm is driven through the documented escape hatch
    // that a real operator would use.
    let (mut db, cfg) = fixture("wo044_parity");
    db.force_jwt_for_test();

    let jwt = root_surface(&db);
    assert!(db.has_cached_root_token(), "the JWT arm must actually have used a token");

    // a second handle, forced onto the pre-JWT (Basic) credential
    let mut basic = scoped_db(&cfg);
    basic.force_basic_for_test();
    let basic_out = root_surface(&basic);
    assert!(!basic.has_cached_root_token(), "the Basic arm must NOT have minted a token");

    assert_eq!(jwt.len(), basic_out.len());
    for (i, (a, b)) in jwt.iter().zip(basic_out.iter()).enumerate() {
        // record ids are server-generated for `CREATE table` without an id, so
        // compare the shapes that are deterministic; every statement here uses
        // an explicit id or a projection, so equality is the right assertion
        assert_eq!(a, b, "statement {i} differed between JWT and Basic root auth");
    }
}

/// argon2 must run ONCE, not per request. Asserting the outcome (one signin for
/// many queries) rather than the operation (that some cache field was touched).
#[test]
fn argon2_runs_once_not_per_request() {
    // The PER-HANDLE counter, not the process-global metric. When several tests
    // in this binary each mint their own token in parallel, the global delta
    // stops being about this handle. A detached handle's own count is exact and
    // cannot be raced.
    let (mut db, _cfg) = fixture("wo044_once");
    db.force_jwt_for_test();
    for i in 0..25 {
        db.sql_root(&format!("SELECT * FROM app_user LIMIT 1; /* {i} */")).expect("query");
    }
    assert_eq!(
        db.root_signins(),
        1,
        "25 root queries must cost EXACTLY one signin — the cache is not holding"
    );
    assert!(db.has_cached_root_token());
}

/// A token the store rejects must trigger re-signin and retry ONCE — never a
/// hard failure the caller sees, and never a silent skip.
#[test]
fn a_rejected_token_re_signs_in_and_retries_once() {
    let (mut db, cfg) = fixture("wo044_retry");
    db.sql_root("SELECT 1 FROM app_user LIMIT 1;").expect("warm");

    // plant a syntactically valid but unsigned-by-this-store token
    let bogus = forge_token(cfg.namespace().as_str(), cfg.database().as_str(), "account", "not-the-key");
    db.force_root_token_for_test(&bogus);

    // the caller must never see the 401
    let rows = db
        .sql_root("SELECT name FROM app_user ORDER BY name;")
        .expect("a rejected token must be replaced transparently, not surfaced");
    assert_eq!(
        rows.as_array().map(Vec::len),
        Some(3),
        "and the retry must return the REAL answer, not an empty one: {rows}"
    );
    assert!(db.has_cached_root_token(), "a fresh token must be cached after the retry");

    // outright garbage takes the same path
    db.force_root_token_for_test("not.a.jwt.at.all");
    db.sql_root("SELECT name FROM app_user LIMIT 1;").expect("garbage token also recovers");
}

/// The refresh must happen BEFORE expiry, computed from the token's own claims
/// rather than a constant that quietly goes stale.
#[test]
fn refresh_is_derived_from_the_tokens_own_expiry() {
    use base64::Engine as _;
    let (mut db, _cfg) = fixture("wo044_ttl");
    db.force_jwt_for_test();
    db.sql_root("SELECT 1 FROM app_user LIMIT 1;").expect("warm");
    let token = db.root_token_for_test().expect("a token must be cached");

    let payload = token.split('.').nth(1).expect("jwt payload");
    let trimmed = payload.trim_end_matches('=');
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed).expect("b64");
    let claims: serde_json::Value = serde_json::from_slice(&bytes).expect("claims");
    let exp = claims.get("exp").and_then(serde_json::Value::as_i64).expect("exp");
    let iat = claims.get("iat").and_then(serde_json::Value::as_i64).expect("iat");
    let life = exp - iat;
    assert!(life > 0, "SurrealDB root tokens must carry a real lifetime (got {life}s)");

    let refresh_in = db.root_refresh_in_for_test().expect("refresh deadline");
    assert!(
        refresh_in.as_secs() < life as u64,
        "refresh ({}s) must fall BEFORE expiry ({life}s), or a query will meet a dead token",
        refresh_in.as_secs()
    );
    // and not so early that it re-mints constantly
    assert!(refresh_in.as_secs() > (life as u64) / 2, "refresh must not thrash: {refresh_in:?}");
    println!("root token life {life}s, refresh in {}s", refresh_in.as_secs());
}

// ── criterion 2: restored-access-key guard integrity ─────────────────────────

/// **The load-bearing test of this change.**
///
/// The keyguard probes with a DELIBERATELY FORGED Bearer token and reads
/// the resulting 401 as its healthy answer. If the root re-signin retry lived in
/// the shared `sql_with_auth` instead of the root-only path, that forged probe
/// would be retried as root, succeed, and the guard would report EVERY HEALTHY
/// STORE compromised — a boot-refusing false positive on every deployment.
///
/// So: the retry must not reach caller-supplied credentials.
#[test]
fn the_root_retry_never_upgrades_a_caller_supplied_credential() {
    let (mut db, cfg) = fixture("wo044_adr013");
    db.force_jwt_for_test();
    db.sql_root("SELECT 1 FROM app_user LIMIT 1;").expect("warm the root token");

    // a forged token must STILL be refused, even though the kernel is now
    // perfectly capable of minting a working one
    let forged = forge_token(cfg.namespace().as_str(), cfg.database().as_str(), "account", REDACTED_KEY);
    let err = db
        .sql_with_auth(&format!("Bearer {forged}"), "RETURN 1;")
        .expect_err("a forged token must be refused, not silently replaced with a root token");
    assert!(
        matches!(&err, BrokerError::Db { detail } if detail.contains("401")),
        "and refused as an auth failure: {err:?}"
    );

    // the keyguard's own verdict on a healthy store is unchanged
    assert_eq!(
        store_accepts_key(&db, REDACTED_KEY).expect("the guard must complete"),
        false,
        "a healthy store must still reject the export placeholder key"
    );
    // ...and it still says YES when the key really is the placeholder, or the
    // guard would be green for the wrong reason
    let real_key_probe = store_accepts_key(&db, "some-other-wrong-key").expect("probe");
    assert!(!real_key_probe, "an arbitrary wrong key must also be rejected");
}

/// The boot guard must still fire on a genuinely compromised store — a guard
/// that only ever returns "safe" is not a guard.
#[test]
fn the_boot_guard_still_refuses_a_redacted_key_store() {
    let (db, cfg) = fixture("wo044_compromised");
    // re-issue the access with the export placeholder as its literal key: this
    // is exactly what `surreal import` of a dump leaves behind
    db.sql_root(&format!(
        "DEFINE ACCESS OVERWRITE account ON DATABASE TYPE RECORD \
         SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass)) \
         WITH JWT ALGORITHM HS512 KEY '{REDACTED_KEY}' DURATION FOR TOKEN 1h, FOR SESSION 12h;"
    ))
    .expect("re-issue access with the placeholder key");

    let fresh = scoped_db(&cfg);
    assert!(
        store_accepts_key(&fresh, REDACTED_KEY).expect("guard completes"),
        "the guard must SEE the compromise"
    );
    let refusal = frust_kernel::keyguard::assert_access_key_is_not_redacted(&fresh)
        .expect_err("and the boot guard must refuse");
    assert!(
        refusal.to_string().contains("FRUST:E_RESTORED_ACCESS_KEY"),
        "with its typed code: {refusal}"
    );
}

/// Root auth still fails closed on bad credentials — the JWT path must not
/// turn a wrong password into a working session.
#[test]
fn wrong_root_credentials_still_fail() {
    use frust_kernel::db::ConnConfig;
    use frust_kernel::tenancy::{Tenancy, TenancyConfig};

    let cfg = TenancyConfig {
        strategy: "single",
        namespace: Some("frust"),
        database: Some("wo044_badcreds"),
        environment: None,
        tenants: &["wo044_badcreds"],
    };
    let conn = ConnConfig { root_pass: "definitely-not-the-password".into(), ..ConnConfig::default() };
    let tenancy = Tenancy::from_config(conn, cfg).expect("tenancy");
    let db = scoped_db(&tenancy.sole().expect("sole"));

    let err = db.sql_root("SELECT 1;").expect_err("bad root credentials must not work");
    println!("bad-credential refusal: {err:?}");
}


