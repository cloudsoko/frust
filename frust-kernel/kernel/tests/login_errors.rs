//! WO-055 G1: a failed login is the caller's problem, a broken store is ours.
//!
//! **The both-sides requirement is the whole test.** Mapping "signin failed"
//! to 401 is easy and half-right; the half that bites is the one where the
//! database is genuinely missing and the kernel cheerfully tells the operator
//! their password is wrong, sending them to the only place the problem isn't.
//!
//! SurrealDB 3.2.0 makes this non-obvious, which is why it is measured rather
//! than assumed — the STATUS cannot tell these apart:
//!
//! | case                     | HTTP | `information`                           |
//! |--------------------------|------|-----------------------------------------|
//! | wrong password / no user | 404  | `No record was returned`                |
//! | database does not exist  | 404  | `The database 'x' does not exist`       |
//! | store present but broken | 400  | `The record access signin query failed` |
//!
//! Both credential rejection and a vanished database are **404**. The
//! discriminant is the body.

mod common;

use frust_kernel::contract::BrokerError;
use frust_kernel::db::scoped_db;
use frust_kernel::meta::{access_ddl, identity_ddl, meta_ddl};
use frust_kernel::tenancy::single_tenant;

fn seeded(name: &str) -> frust_kernel::db::Db {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!("{}; {}; {}", meta_ddl(), identity_ddl(), access_ddl())).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', \
         pass = crypto::argon2::generate('pw-mgr');",
    )
    .unwrap();
    db
}

/// Side one: the credentials are wrong.
#[test]
fn a_wrong_password_is_a_typed_rejection_not_a_server_error() {
    let db = seeded("wo055_creds");

    let err = db.signin("mgr", "definitely-not-the-password").expect_err("must refuse");
    println!("wrong password -> {err:?}");
    match &err {
        BrokerError::PermissionDenied { detail } => assert_eq!(
            detail,
            frust_kernel::db::AUTH_REJECTED,
            "a rejected login must carry the typed marker, not prose"
        ),
        other => panic!("a wrong password must be PermissionDenied, got {other:?}"),
    }

    // and it must not carry the transport's own words to an unauthenticated
    // caller — the leak half of G1
    let text = format!("{err:?}");
    assert!(!text.contains("http status"), "leaks the transport detail: {text}");
    assert!(!text.contains("404"), "leaks SurrealDB's status: {text}");

    // an unknown user is the same answer (and must not be distinguishable —
    // "no such user" vs "wrong password" is a user-enumeration oracle)
    let ghost = db.signin("no_such_user", "x").expect_err("must refuse");
    assert_eq!(
        format!("{ghost:?}"),
        format!("{err:?}"),
        "unknown-user and wrong-password must be indistinguishable"
    );

    // the control: the same call with the RIGHT password still works, so the
    // refusals above are the classifier and not a broken signin path
    db.signin("mgr", "pw-mgr").expect("correct credentials still sign in");
}

/// Side two — **the half that makes the fix correct**: the store is missing,
/// and that is emphatically not the caller's password.
#[test]
fn a_missing_database_is_still_a_server_error() {
    // never created: SurrealDB answers 404 exactly like a bad password does,
    // and only the body distinguishes them
    let cfg = single_tenant(&"wo055_no_such_db".to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns("REMOVE DATABASE IF EXISTS wo055_no_such_db;").ok();

    let err = db.signin("mgr", "pw-mgr").expect_err("cannot sign in to a database that isn't there");
    println!("missing database -> {err:?}");
    match &err {
        BrokerError::Db { detail } => {
            assert!(
                !detail.contains(frust_kernel::db::AUTH_REJECTED),
                "a missing database must NOT be reported as rejected credentials: {detail}"
            );
            println!("  reported as a server fault, saying which: {detail}");
        }
        BrokerError::PermissionDenied { .. } => panic!(
            "THE BOTH-SIDES FAILURE: a vanished database was reported as a credential \
             rejection. An operator told 'your password is wrong' will go looking at \
             the one thing that is fine."
        ),
        other => panic!("expected a Db fault, got {other:?}"),
    }
}

/// The classifier itself, over the responses actually recorded from SurrealDB
/// 3.2.0 — so the table in this file's header is executable rather than a
/// comment that rots.
#[test]
fn the_classifier_matches_the_recorded_responses() {
    let cases: &[(u16, &str, bool, &str)] = &[
        (404, r#"{"code":404,"details":"Not found","information":"No record was returned"}"#, true,
         "wrong password / unknown user"),
        (404, r#"{"code":404,"details":"Not found","information":"The database 'x' does not exist"}"#, false,
         "database missing"),
        (404, r#"{"code":404,"details":"Not found","information":"The namespace 'x' does not exist"}"#, false,
         "namespace missing"),
        (400, r#"{"code":400,"information":"The record access signin query failed"}"#, false,
         "store present but broken"),
        (401, r#"{"code":401,"details":"Authentication failed"}"#, false,
         "malformed request"),
        (500, "There was a problem with the database", false, "not even JSON"),
    ];
    for (status, body, expect_rejection, what) in cases {
        let got = frust_kernel::db::is_credential_rejection_for_test(*status, body);
        println!("  {status} {what:<34} -> rejection={got} (want {expect_rejection})");
        assert_eq!(
            got, *expect_rejection,
            "misclassified '{what}': a false TRUE tells a user their password is wrong when \
             the store is broken; a false FALSE tells them the server is broken when they \
             simply typed it wrong"
        );
    }
}
