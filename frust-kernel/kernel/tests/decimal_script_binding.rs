//! WO-030: the `Decimal` binding in the script host — `decimal.rs` compiled
//! into the Boa sandbox verbatim, so an app author computes exact money instead
//! of hand-rolling it.
//!
//! The load-bearing property is **three hosts, one answer** (criterion 3): a
//! script, the kernel's `decimal.rs`, and SurrealDB's own decimal all produce
//! the *same* string for `qty × rate`. This extends WO-021's byte-equal
//! reconciliation from two hosts to three — and because the binding IS
//! `decimal.rs` (compiled into the guest, not reimplemented in JS), agreement is
//! by construction, not by luck.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::single_tenant;
use frust_kernel::decimal::{Decimal, Mode};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// A doctype with a currency `out` field and a script that writes into it — the
/// minimal rig to observe what the binding computes, end to end into the DB.
fn broker_running(script: &str, dbname: &str) -> (Arc<Broker>, Db) {
    let cfg = single_tenant(&dbname.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {dbname}; DEFINE DATABASE {dbname};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:u SET name = 'u', role = 'manager', pass = crypto::argon2::generate('pw-u'); \
         DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE doctype CONTENT {{ name: 'calc', fields: [ \
            {{ fieldname: 'qty', fieldtype: 'Data' }}, \
            {{ fieldname: 'rate', fieldtype: 'Currency' }}, \
            {{ fieldname: 'note', fieldtype: 'Data' }}, \
            {{ fieldname: 'out', fieldtype: 'Currency' }} ] }};",
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

/// Create a `calc` doc with qty/rate; the script fills `out`. Returns the
/// broker's returned doc (or the typed error).
fn compute(b: &Broker, qty: &str, rate: &str) -> Result<serde_json::Value, BrokerError> {
    b.db_write(
        &caller(),
        &HookChain::default(),
        WriteOp::Create,
        "calc",
        None,
        &[
            ("qty".to_string(), Value::Text(qty.into())),
            ("rate".to_string(), Value::Decimal(rate.into())),
            ("note".to_string(), Value::Text("".into())),
            ("out".to_string(), Value::Decimal("0".into())),
        ],
    )
}

/// The kernel host's answer for `qty × rate` rounded to cents, half-even — the
/// reference the other two hosts must match.
fn kernel_line(qty: &str, rate: &str) -> String {
    Decimal::parse(qty)
        .unwrap()
        .mul(Decimal::parse(rate).unwrap())
        .unwrap()
        .round(2, Mode::HalfEven)
        .to_plain_string()
}

// The script writes its answer into BOTH `out` (Currency — the DB value
// channel) and `note` (Data — the raw string, which SurrealDB will not
// normalize), so one write proves string-agreement AND value-agreement.
const LINE_SCRIPT: &str = r#"
    doc.out = Decimal.round(Decimal.mul(doc.qty, doc.rate), 2, "half_even");
    doc.note = doc.out;
"#;

/// Criterion 3 — THREE HOSTS, ONE ANSWER. For a spread of qty×rate cases,
/// including the `0.335` family that bit the PM's hand-rolled version, the
/// script host, the kernel's `decimal.rs`, and SurrealDB's decimal all agree.
///
/// String-agreement is asserted off `note` (a Data field): SurrealDB strips a
/// decimal's trailing zeros (`1.00` → `1`), so the literal string only survives
/// on the text field, while the money field is checked by DB decimal EQUALITY —
/// value, not representation (the WO-016 discipline, made twice-over here).
#[test]
fn three_hosts_one_answer() {
    let (b, db) = broker_running(LINE_SCRIPT, "dsb_three");

    let cases = [("2.5", "19.99"), ("3", "0.335"), ("7", "1.014"), ("1", "0.005")];
    for (qty, rate) in cases {
        let out = compute(&b, qty, rate).expect("script computes the line");
        let script_ans = out["note"].as_str().unwrap().to_string(); // RAW, un-normalized
        let kernel_ans = kernel_line(qty, rate);

        // host 1 (script raw string) == host 2 (kernel decimal.rs string)
        assert_eq!(
            script_ans, kernel_ans,
            "script and kernel disagree on {qty} × {rate}: script={script_ans} kernel={kernel_ans}"
        );

        // host 3 (the DB): the stored money equals the kernel answer as a DB
        // decimal — asked of SurrealDB, not string-compared, and never epsilon.
        let key = out["id"].as_str().unwrap().split_once(':').unwrap().1.to_string();
        let rows = db
            .sql_root(&format!(
                "SELECT out = {kernel_ans}dec AS exact, <string>out AS shown, \
                 type::is_decimal(out) AS d FROM calc:{key};"
            ))
            .unwrap();
        let row = &rows.as_array().unwrap()[0];
        assert_eq!(row["exact"], serde_json::json!(true),
            "DB disagrees on {qty} × {rate}: stored {} vs kernel {kernel_ans}", row["shown"]);
        assert_eq!(row["d"], serde_json::json!(true), "stored as a real decimal");
        println!("{qty} × {rate} = {script_ans}  (all three hosts agree)");
    }
}

/// Criterion 4 — the exact scale bug that bit the PM. The hand-rolled
/// fixed-scale parse truncated rate `0.335` to `0.33`, giving `3 × 0.33 = 0.99`.
/// The binding keeps full precision: `3 × 0.335 = 1.005 → 1.00`. This asserts
/// the RIGHT answer *and names the wrong one it is not*. Read off `note` so the
/// scale-2 string `1.00` is visible (the DB would show it as `1`).
#[test]
fn the_scale_bug_cannot_recur() {
    let (b, _db) = broker_running(LINE_SCRIPT, "dsb_scalebug");
    let out = compute(&b, "3", "0.335").expect("computes");
    let ans = out["note"].as_str().unwrap();
    assert_eq!(ans, "1.00", "3 × 0.335 = 1.005 → 1.00 half-even, exact");
    assert_ne!(ans, "0.99", "0.99 is the truncate-rate-to-0.33 bug; it must not recur");
    // and the kernel agrees this is the right answer, not just a matching wrong one
    assert_eq!(ans, kernel_line("3", "0.335"));
}

/// Criterion 1 — the API surface works: add/sub/mul/div/round/cmp, exact.
#[test]
fn the_binding_api_is_exact() {
    // a script that exercises every op and concatenates the results, so one
    // script proves the whole surface in one write.
    let script = r#"
        var parts = [
            Decimal.add("0.1", "0.2"),          // 0.3  (the classic float miss)
            Decimal.sub("0.30", "0.1"),         // 0.20
            Decimal.mul("1.25", "1.25"),        // 1.5625 (exact, scale grows)
            Decimal.div("1", "3", 2, "half_even"),   // 0.33
            Decimal.round("2.125", 2, "half_even"),  // 2.12 (to even)
            Decimal.round("2.125", 2, "half_up"),    // 2.13 (away)
            String(Decimal.cmp("1.50", "1.5")), // 0 (numeric equality)
            String(Decimal.cmp("10", "9.99"))   // 1
        ];
        doc.out = "0"; doc.note = parts.join("|");
    "#;
    // reuse the calc rig but read the note back through a Data field
    let (b, _db) = {
        let dbname = "dsb_api";
        let cfg = single_tenant(&dbname.to_string()).expect("tenancy");
        let db = scoped_db(&cfg);
        db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {dbname}; DEFINE DATABASE {dbname};")).unwrap();
        db.sql_root(&format!(
            "{}; {}; CREATE app_user:u SET name='u', role='manager', pass=crypto::argon2::generate('pw-u'); \
             DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
             CREATE doctype CONTENT {{ name:'calc', fields:[ \
                {{fieldname:'out', fieldtype:'Currency'}}, {{fieldname:'note', fieldtype:'Data'}} ] }};",
            identity_ddl(), access_ddl(),
        )).unwrap();
        frust_kernel::sync::MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
        let hooks = WasmHooks::load_with_script(artifacts(), script).unwrap();
        (Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks))), db)
    };
    let out = b.db_write(&caller(), &HookChain::default(), WriteOp::Create, "calc", None, &[
        ("out".into(), Value::Decimal("0".into())),
        ("note".into(), Value::Text("".into())),
    ]).expect("api script runs");
    let got = out["note"].as_str().unwrap();
    assert_eq!(got, "0.3|0.20|1.5625|0.33|2.12|2.13|0|1", "full surface exact: {got}");
}

/// Criterion 2 — rounding stays EXPLICIT. `mul` never rounds: `3 × 0.335` left
/// unrounded is `1.005` (scale 4-ish), NOT `1.00`. An author who wants cents
/// must `round`, exactly as `decimal.rs` requires. The binding adds a safe path;
/// it does not round behind the author's back.
#[test]
fn mul_does_not_round_implicitly() {
    let script = r#"doc.out = Decimal.mul(doc.qty, doc.rate);"#; // no round()
    let (b, _db) = broker_running(script, "dsb_noround");
    let out = compute(&b, "3", "0.335").expect("computes");
    assert_eq!(out["out"].as_str().unwrap(), "1.005", "mul is exact and unrounded");
}

/// Criterion 2 (guard intact) — the WO-017 decimal-catch still fires WITH the
/// binding present. A script that does bare JS float math on money is still
/// refused; adding `Decimal` did not open a float back door.
#[test]
fn the_float_catch_still_bites_with_the_binding_present() {
    let (b, db) = broker_running("doc.out = Number(doc.rate) + 0.2;", "dsb_guard");
    let err = compute(&b, "1", "0.1").expect_err("bare float math on money is still refused");
    let msg = format!("{err:?}");
    assert!(msg.contains("FRUST:E_MONEY_FLOAT"), "the guard still fires: {msg}");
    let n = db.sql_root("SELECT count() AS n FROM calc GROUP ALL;").unwrap();
    let n = n.as_array().and_then(|a| a.first()).and_then(|r| r["n"].as_u64()).unwrap_or(0);
    assert_eq!(n, 0, "rejected write did not land");
}

/// Criterion 1 (typed failure) — money errors cross the binding as typed
/// rejections, never a silent NaN or wrap. Division by zero is refused with its
/// machine code.
#[test]
fn division_by_zero_is_a_typed_reject() {
    let (b, _db) = broker_running(r#"doc.out = Decimal.div(doc.rate, "0", 2, "half_even");"#, "dsb_divzero");
    let err = compute(&b, "1", "5.00").expect_err("div by zero must be refused");
    let msg = format!("{err:?}");
    assert!(msg.contains("FRUST:E_MONEY_DIVBYZERO"), "typed div-by-zero: {msg}");
}

/// Criterion 6 — CONTAINMENT INTACT. The binding is pure arithmetic, no new
/// capability. A script that calls `Decimal` inside an infinite loop is still
/// killed by the fuel/epoch guard — the sandbox bounds compute whether or not
/// the doc touches money.
#[test]
fn decimal_in_a_hot_loop_is_still_contained() {
    let (b, _db) = broker_running(
        r#"var x = "0"; while (true) { x = Decimal.add(x, "0.01"); } doc.out = x;"#,
        "dsb_contain",
    );
    let err = compute(&b, "1", "1.00").expect_err("an infinite loop must be killed, binding or not");
    let msg = format!("{err:?}");
    println!("contained: {msg}");
    assert!(matches!(err, BrokerError::HookRejected { .. }), "typed containment, got {msg}");
}
