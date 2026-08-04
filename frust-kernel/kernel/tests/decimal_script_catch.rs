//! The decimal NaN-catch, proven in the KERNEL host.
//!
//! The shell records each field's WIT variant kind on the way in and
//! re-imposes it on the way out. This asserts the other half of that contract:
//! the shell REFUSING to re-impose what a script corrupted. Money that a
//! script turned into a float, a NaN, or another type is rejected loudly and
//! never stored (REQ-6.2.1).
//!
//! The engine lives in the shared artifact, so every case here holds in the
//! browser host too — verified separately in the Desk.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// The engine reads its script from FRUST_SCRIPT — the same seam the browser
/// host fills via `_setEnv`, here handed to the guest as the single variable
/// in an otherwise EMPTY environment (the kernel never inherits its own).
fn broker_running(script: &str, dbname: &str) -> (Arc<Broker>, Db) {
    let cfg = single_tenant(&dbname.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {dbname}; DEFINE DATABASE {dbname};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:u SET name = 'u', role = 'manager', pass = crypto::argon2::generate('pw-u'); \
         DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE doctype CONTENT {{ name: 'money', fields: [ \
            {{ fieldname: 'amount', fieldtype: 'Currency' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    frust_kernel::sync::MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    let hooks = WasmHooks::load_with_script(artifacts(), script).unwrap();
    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)));
    (b, db)
}

fn caller() -> Caller {
    Caller { user: "u".into(), pass: "pw-u".into(), role: "manager".into() }
}

fn write_amount(b: &Broker, amount: &str) -> Result<serde_json::Value, BrokerError> {
    b.db_write(
        &caller(),
        &HookChain::default(),
        WriteOp::Create,
        "money",
        None,
        &[("amount".to_string(), Value::Decimal(amount.into()))],
    )
}

/// The catch: float arithmetic on money is refused, and nothing is stored.
#[test]
fn float_mangled_currency_is_rejected_and_not_stored() {
    // the classic: 0.1 + 0.2 is not 0.3 in binary floating point
    let (b, db) = broker_running(
        "doc.amount = Number(doc.amount) + 0.2;",
        "dsc_float",
    );
    let err = write_amount(&b, "0.1").expect_err("a float-mangled decimal must be refused");
    let msg = format!("{err:?}");
    println!("reject: {msg}");
    assert!(matches!(err, BrokerError::HookRejected { .. }), "typed rejection, got {msg}");
    assert!(msg.contains("FRUST:E_MONEY_FLOAT"), "stable machine code in the message: {msg}");

    // NEVER STORED is the load-bearing half — a loud error that still wrote
    // the mangled value would be worse than silence.
    let rows = db.sql_root("SELECT count() AS n FROM money GROUP ALL;").unwrap();
    let n = rows.as_array().and_then(|a| a.first()).and_then(|r| r["n"].as_u64()).unwrap_or(0);
    assert_eq!(n, 0, "the rejected write must not have landed");
}

/// NaN must not reach the shell disguised as null: `JSON.stringify` turns NaN
/// into `null`, which is indistinguishable from a deliberate clear. The catch
/// runs BEFORE stringify, so the two stay distinguishable.
#[test]
fn nan_is_caught_before_stringify_erases_it() {
    let (b, _db) = broker_running("doc.amount = Number(\"not a number\");", "dsc_nan");
    let err = write_amount(&b, "10.00").expect_err("NaN must be refused");
    let msg = format!("{err:?}");
    println!("reject: {msg}");
    assert!(msg.contains("FRUST:E_FIELD_NAN"), "NaN caught as NaN, not as null: {msg}");
}

/// A script replacing money with a different type is a type change, not a value.
#[test]
fn type_change_on_currency_is_rejected() {
    let (b, _db) = broker_running("doc.amount = { total: 5 };", "dsc_type");
    let err = write_amount(&b, "10.00").expect_err("an object is not money");
    let msg = format!("{err:?}");
    println!("reject: {msg}");
    assert!(msg.contains("FRUST:E_MONEY_TYPE"), "typed as a type error: {msg}");
}

/// A non-numeric string is refused rather than handed to the database.
#[test]
fn non_numeric_string_on_currency_is_rejected() {
    let (b, _db) = broker_running("doc.amount = \"twelve euros\";", "dsc_str");
    let err = write_amount(&b, "10.00").expect_err("prose is not money");
    let msg = format!("{err:?}");
    println!("reject: {msg}");
    assert!(msg.contains("FRUST:E_MONEY_NOT_NUMERIC"), "typed as non-numeric: {msg}");
}

/// The DOCUMENTED SAFE PATH must actually work: round explicitly, write back a
/// string, and the shell re-imposes the decimal exactly. This is the assertion
/// that stops the catch from being merely restrictive.
#[test]
fn the_documented_safe_path_is_accepted_and_exact() {
    let (b, db) = broker_running(
        "var v = Number(doc.amount) * 3; doc.amount = v.toFixed(2);",
        "dsc_safe",
    );
    let out = write_amount(&b, "10.10").expect("the safe path must be accepted");
    println!("stored: {}", out["amount"]);

    // decimal-equal, asked of the DB — not string-equal, and never epsilon
    let rows = db
        .sql_root("SELECT <string>amount AS shown, amount = 30.30dec AS exact, \
                   type::is_decimal(amount) AS d FROM money;")
        .unwrap();
    let row = &rows.as_array().unwrap()[0];
    assert_eq!(row["exact"], serde_json::json!(true), "10.10 x 3 = exactly 30.30 (shown {})", row["shown"]);
    assert_eq!(row["d"], serde_json::json!(true), "and it is stored as a decimal");
}

/// An integral number is exactly representable, so it round-trips rather than
/// being refused — the rule is about float error, not about JSON types.
#[test]
fn integral_number_on_currency_is_accepted() {
    let (b, db) = broker_running("doc.amount = 42;", "dsc_int");
    b.db_write(
        &caller(),
        &HookChain::default(),
        WriteOp::Create,
        "money",
        None,
        &[("amount".to_string(), Value::Decimal("1.00".into()))],
    )
    .expect("an exact integer is not a mangled float");
    let rows = db.sql_root("SELECT amount = 42dec AS exact FROM money;").unwrap();
    assert_eq!(rows.as_array().unwrap()[0]["exact"], serde_json::json!(true));
}

/// Clearing a field stays legal — the catch must not make null impossible,
/// or "corrupted" and "cleared" collapse into each other from the other side.
#[test]
fn explicit_null_on_currency_is_still_allowed() {
    let (b, _db) = broker_running("doc.amount = null;", "dsc_null");
    b.db_write(
        &caller(),
        &HookChain::default(),
        WriteOp::Create,
        "money",
        None,
        &[("amount".to_string(), Value::Decimal("5.00".into()))],
    )
    .expect("clearing money is a legitimate operation");
}
