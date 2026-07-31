//! WO-027 — the restored-signing-key guard, and the canary that keeps it honest.
//!
//! `surreal export` redacts the record access's JWT signing key to the literal
//! `[REDACTED]`; `surreal import` restores that string AS the key. The result
//! boots, logs in, and serves — while accepting tokens anyone can forge. The
//! guard refuses to serve such a store.
//!
//! **The canary is the load-bearing test here.** The guard's correctness rests
//! on a SurrealDB constant. A version that changes the redaction string would
//! silently disable the guard and quietly reopen an auth bypass — the worst
//! kind of regression, because everything would still look green. So the
//! constant is pinned and asserted against the live server (WO-018
//! conflict-string precedent).
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::keyguard::{assert_access_key_is_not_redacted, store_accepts_key, REDACTED_KEY};
use frust_kernel::meta::{access_ddl, identity_ddl};

fn setup(name: &str, key: Option<&str>) -> Db {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!("{};", identity_ddl())).unwrap();
    match key {
        // a store as `surreal import` leaves it: the placeholder AS the key
        Some(k) => db
            .sql_root(&format!(
                "DEFINE ACCESS OVERWRITE account ON DATABASE TYPE RECORD \
                 SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass)) \
                 WITH JWT ALGORITHM HS512 KEY '{k}' DURATION FOR TOKEN 1h, FOR SESSION 12h;"
            ))
            .unwrap(),
        // a healthy store: SurrealDB generates its own secret
        None => db.sql_root(&format!("{};", access_ddl())).unwrap(),
    };
    db
}

/// THE CANARY. If a SurrealDB upgrade changes what `export` writes in place of
/// a secret, this fails — loudly — instead of the guard silently going blind.
///
/// Asserted as a PROPERTY of the running server, not a string in our source:
/// a store whose key IS the pinned constant must accept a token signed with
/// it, and a store with a real key must not. If either half stops holding, the
/// detection mechanism no longer detects.
#[test]
fn the_redaction_constant_still_discriminates() {
    let vulnerable = setup("kg_canary_bad", Some(REDACTED_KEY));
    assert!(
        store_accepts_key(&vulnerable, REDACTED_KEY).expect("probe the vulnerable store"),
        "a store keyed with '{REDACTED_KEY}' must accept a token signed with it — \
         if this fails, SurrealDB's JWT handling changed and the guard is blind"
    );

    let healthy = setup("kg_canary_good", None);
    assert!(
        !store_accepts_key(&healthy, REDACTED_KEY).expect("probe the healthy store"),
        "a store with a real key must REJECT the published constant — \
         if this fails, the guard would refuse every healthy database"
    );
}

/// The guard refuses a restored store.
#[test]
fn the_guard_refuses_a_restored_store() {
    let db = setup("kg_refuse", Some(REDACTED_KEY));
    let err = assert_access_key_is_not_redacted(&db).expect_err("must refuse to serve");
    let msg = format!("{err:?}");
    println!("refused: {}", &msg[..msg.len().min(160)]);
    assert!(msg.contains("FRUST:E_RESTORED_ACCESS_KEY"), "stable machine code: {msg}");
    assert!(msg.contains("DEFINE ACCESS OVERWRITE"), "the refusal names the remedy: {msg}");
}

/// And does NOT refuse a healthy one — the false-positive direction, which
/// would be just as damaging in its own way (a guard that cries wolf gets
/// disabled by the first operator it inconveniences).
#[test]
fn the_guard_allows_a_healthy_store() {
    let db = setup("kg_allow", None);
    assert_access_key_is_not_redacted(&db).expect("a healthy store must serve");
}

/// Re-issuing the access with a fresh secret is the ONLY exit, and it works —
/// using the EXACT remedy the guard's message prints.
///
/// Note what this test caught: `access_ddl()` is `DEFINE ACCESS IF NOT EXISTS`
/// (WO-008, so an ordinary boot never rotates the key and never signs everyone
/// out). On a restored store that is a **no-op** — the placeholder key
/// survives it. The operator must use `OVERWRITE` explicitly, which is why the
/// guard's message spells that out rather than saying "re-run the DDL".
#[test]
fn re_issuing_the_key_clears_the_refusal() {
    let db = setup("kg_remedy", Some(REDACTED_KEY));
    assert!(assert_access_key_is_not_redacted(&db).is_err(), "refused before remediation");

    // the IF NOT EXISTS form does NOT fix it — proving the runbook needs OVERWRITE
    db.sql_root(&format!("{};", access_ddl())).unwrap();
    assert!(
        assert_access_key_is_not_redacted(&db).is_err(),
        "IF NOT EXISTS is a no-op here — a runbook that said 're-run access_ddl' would be WRONG"
    );

    // the documented remedy
    db.sql_root(
        "DEFINE ACCESS OVERWRITE account ON DATABASE TYPE RECORD \
         SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass)) \
         WITH JWT ALGORITHM HS512 KEY 'a-fresh-high-entropy-secret-for-this-test-only' \
         DURATION FOR TOKEN 1h, FOR SESSION 12h;",
    )
    .unwrap();
    assert_access_key_is_not_redacted(&db).expect("serves after the key is re-issued");
    assert!(
        !store_accepts_key(&db, REDACTED_KEY).expect("probe"),
        "and the forged token no longer works"
    );
}
