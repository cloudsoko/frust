//! WO-022: the accounting seed — the platform dogfooded.
//!
//! A real (minimal) accounting app, shipped as a BUNDLE, running on machinery
//! none of which was written for accounting: DocTypes, an approval workflow,
//! WO-021 money arithmetic, a WO-017 server script, a WO-007/016 AR rollup,
//! the WO-019 lifecycle. No core changes — if a criterion needs one, that is a
//! FINDING, recorded in the build log, not patched into the kernel to make the
//! demo green.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::meta_ddl;
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// The reconciliation + line-computation server script.
///
/// **WO-030: the footgun is CLOSED.** This once hand-rolled decimal multiply and
/// half-even rounding in integer minor units — exact, but the very float-money
/// footgun the platform exists to remove, re-handed to every app author (and my
/// own first fixed-scale version truncated rate `0.335` to `0.33`). It now calls
/// the `Decimal` binding, which is `decimal.rs` compiled into the script engine
/// verbatim — the SAME arithmetic the kernel and DB reconcile against. Money
/// crosses as strings; `mul` is exact and rounding is explicit at each defined
/// point. The scale bug cannot recur because there is no hand-rolled parse to
/// lose precision (proven in [[decimal_script_binding.rs]]).
const RECON_SCRIPT: &str = r#"
var lines = doc.lines || [];
var total = "0";
for (var i = 0; i < lines.length; i++) {
  // qty × rate EXACT, then rounded to cents at the line (a defined point)
  var amt = Decimal.round(Decimal.mul(lines[i].qty, lines[i].rate), 2, "half_even");
  lines[i].amount = amt;
  total = Decimal.add(total, amt);
}
var stated = Decimal.round(doc.total, 2, "half_even");
if (Decimal.cmp(stated, total) !== 0) {
  throw new Error("Invoice does not balance: lines sum to " + total
    + " but total is " + stated + " [FRUST:E_INVOICE_UNBALANCED]");
}
doc.lines = lines;
"#;

fn seed_bundle() -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1,
        "name": "acct",
        "version": "1.0.0",
        "label": "Accounting",
        "doctypes": [
            { "name": "customer", "fields": [
                { "fieldname": "cust_name", "fieldtype": "Data", "required": true } ] },
            { "name": "item", "fields": [
                { "fieldname": "item_name", "fieldtype": "Data", "required": true },
                { "fieldname": "rate", "fieldtype": "Currency" } ] },
            { "name": "sales_invoice", "submittable": true, "fields": [
                { "fieldname": "customer", "fieldtype": "Data", "required": true },
                { "fieldname": "lines", "fieldtype": "Table", "options": ["invoice_line"] },
                { "fieldname": "total", "fieldtype": "Currency", "required": true },
                { "fieldname": "workflow_state", "fieldtype": "Data" } ],
              // AR: invoices CHARGE the customer (Tier-1 counter, decimal)
              "aggregates": [ { "kind": "counter", "rollup": "ar_outstanding",
                "key": "customer", "metrics": [ { "name": "charged", "field": "total" } ] } ] },
            { "name": "invoice_line", "fields": [
                { "fieldname": "item", "fieldtype": "Data" },
                { "fieldname": "qty", "fieldtype": "Data" },
                { "fieldname": "rate", "fieldtype": "Currency" },
                { "fieldname": "amount", "fieldtype": "Currency" } ] },
            { "name": "payment", "submittable": true, "fields": [
                { "fieldname": "customer", "fieldtype": "Data", "required": true },
                { "fieldname": "amount", "fieldtype": "Currency", "required": true } ],
              // payments PAY DOWN AR (a second metric on the same rollup —
              // outstanding = charged - paid, computed at read; both positive)
              "aggregates": [ { "kind": "counter", "rollup": "ar_outstanding",
                "key": "customer", "metrics": [ { "name": "paid", "field": "amount" } ] } ] },
            // the AR rollup record itself is a readable DocType (WO-007)
            { "name": "ar_outstanding", "fields": [
                { "fieldname": "k", "fieldtype": "Data" },
                { "fieldname": "charged", "fieldtype": "Currency" },
                { "fieldname": "paid", "fieldtype": "Currency" } ] }
        ],
        "server_scripts": [
            { "doctype": "sales_invoice", "hook": "validate", "script": RECON_SCRIPT } ],
        "workflows": [ {
            "name": "invoice_approval",
            "doctype": "sales_invoice",
            "states": [
                { "name": "Draft", "docstatus": 0 },
                { "name": "Submitted for Approval", "docstatus": 0 },
                { "name": "Approved", "docstatus": 1 },
                { "name": "Rejected", "docstatus": 0 } ],
            "transitions": [
                { "from": "Draft", "to": "Submitted for Approval", "role": "clerk", "action": "Submit" },
                { "from": "Submitted for Approval", "to": "Approved", "role": "manager", "action": "Approve" },
                { "from": "Submitted for Approval", "to": "Rejected", "role": "manager", "action": "Reject" },
                { "from": "Rejected", "to": "Draft", "role": "clerk", "action": "Reopen" } ]
        } ]
    })
}

const DB: &str = "acct_seed";

fn setup() -> (Rest, Arc<Broker>, ResolvedTenant) {
    let cfg = single_tenant(&DB.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {DB}; DEFINE DATABASE {DB};")).unwrap();
    db.sql_root(&meta_ddl()).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', pass = crypto::argon2::generate('pw-mgr'); \
         CREATE app_user:clerk SET name = 'clerk', role = 'clerk', pass = crypto::argon2::generate('pw-clerk');",
    )
    .unwrap();
    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source(scoped_db(&cfg));
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)));
    let rest = Rest::single(Arc::clone(&broker), "127.0.0.1:0".into(), Some(Arc::new(MetadataSync { base: cfg.clone() })), None);
    (rest, broker, cfg)
}

fn mgr() -> Caller { Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() } }
fn clerk() -> Caller { Caller { user: "clerk".into(), pass: "pw-clerk".into(), role: "clerk".into() } }

fn call(rest: &Rest, path: &str, who: &Caller, body: serde_json::Value) -> Result<serde_json::Value, BrokerError> {
    rest.route_for_test(path, &body, who)
}

/// Criteria 1-4: install the bundle, run the invoice lifecycle, the money
/// reconciles exactly, AR moves by the decimal amount.
#[test]
fn the_accounting_seed_runs_as_an_app() {
    let (rest, broker, cfg) = setup();

    // ── 1. INSTALL from the bundle (no core changes) ──
    let out = call(&rest, "/app/install", &mgr(), seed_bundle()).expect("install the seed");
    println!("installed: {out}");
    assert_eq!(out["action"], serde_json::json!("installed"));

    // a fresh broker sees the installed server script via the script source
    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source(scoped_db(&cfg));
    let b = Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)));

    // ── 2. clerk creates an invoice with lines; qty × rate computed exactly ──
    // 2.5 × 19.99 = 49.975 → 49.98 ; 3 × 0.335 = 1.005 → 1.00 (half-even)
    let lines = Value::List(vec![
        Value::Object(vec![
            ("item".into(), Value::Text("widget".into())),
            ("qty".into(), Value::Text("2.5".into())),
            ("rate".into(), Value::Decimal("19.99".into())),
        ]),
        Value::Object(vec![
            ("item".into(), Value::Text("penny".into())),
            ("qty".into(), Value::Text("3".into())),
            ("rate".into(), Value::Decimal("0.335".into())),
        ]),
    ]);
    let inv = b.db_write(&clerk(), &HookChain::default(), WriteOp::Create, "sales_invoice", None, &[
        ("customer".into(), Value::Text("acme".into())),
        ("lines".into(), lines.clone()),
        ("total".into(), Value::Decimal("50.98".into())), // 49.98 + 1.00
        ("workflow_state".into(), Value::Text("Draft".into())),
    ]).expect("create invoice");
    let key = inv["id"].as_str().unwrap().split_once(':').unwrap().1.to_string();
    println!("invoice: {}", serde_json::to_string(&inv).unwrap());

    // the server script computed the line amounts exactly
    let stored = broker.db.sql_root(&format!("SELECT lines FROM sales_invoice:{key};")).unwrap();
    let text = stored.to_string();
    assert!(text.contains("49.98"), "line 1 = 2.5 × 19.99 = 49.98: {text}");
    assert!(text.contains("1.00") || text.contains("\"amount\":\"1.00\""), "line 2 = 3 × 0.335 = 1.00 (half-even): {text}");

    // ── the reconciliation refuses a deliberately unbalanced invoice ──
    let bad = b.db_write(&clerk(), &HookChain::default(), WriteOp::Create, "sales_invoice", None, &[
        ("customer".into(), Value::Text("acme".into())),
        ("lines".into(), lines.clone()),
        ("total".into(), Value::Decimal("99.99".into())), // wrong
        ("workflow_state".into(), Value::Text("Draft".into())),
    ]).expect_err("an unbalanced invoice must be refused");
    println!("unbalanced refused: {bad:?}");
    assert!(format!("{bad:?}").contains("E_INVOICE_UNBALANCED"), "{bad:?}");

    // ── 3. workflow: clerk submits (0→0), manager approves (0→1) ──
    //
    // WO-028: assert the WHOLE DOCUMENT survives each state change, not just
    // the field under test. This test previously checked AR (derived from
    // `total`, which happened to be unaffected) and never re-read `lines` —
    // so a transition silently destroying the embedded child rows shipped
    // through the entire platform milestone. A test that only checks what it
    // changed cannot catch what it silently destroyed.
    let lines_len = |b: &Broker| -> usize {
        b.db.sql_root(&format!("SELECT VALUE array::len(lines) FROM sales_invoice:{key};"))
            .unwrap()
            .as_array()
            .and_then(|a| a.first())
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0) as usize
    };
    assert_eq!(lines_len(&broker), 2, "two lines before any transition");

    b.db_transition(&clerk(), &HookChain::default(), "sales_invoice", &key, "Submit").expect("submit");
    assert_eq!(lines_len(&broker), 2, "lines survive the clerk's Submit (0->0)");

    let approved = b.db_transition(&mgr(), &HookChain::default(), "sales_invoice", &key, "Approve").expect("approve");
    assert_eq!(approved["docstatus"], serde_json::json!(1), "approved invoice is docstatus 1");
    assert_eq!(lines_len(&broker), 2, "lines survive the manager's Approve (0->1)");

    // ── 4. AR: the counter charged the customer by the exact total ──
    // counters fire on docstatus = 1, so AR moved on approval
    let ar = broker.db.sql_root("SELECT <string>charged AS charged, <string>paid AS paid FROM ar_outstanding:acme;").unwrap();
    let ar = &ar.as_array().unwrap()[0];
    println!("AR after approve: charged={} paid={}", ar["charged"], ar["paid"]);
    assert_eq!(ar["charged"].as_str(), Some("50.98"), "AR charged = invoice total, exact decimal");

    // a payment pays down AR; outstanding = charged - paid
    let pay = b.db_write(&clerk(), &HookChain::default(), WriteOp::Create, "payment", None, &[
        ("customer".into(), Value::Text("acme".into())),
        ("amount".into(), Value::Decimal("20.98".into())),
    ]).expect("create payment");
    let pkey = pay["id"].as_str().unwrap().split_once(':').unwrap().1.to_string();
    b.db_transition(&mgr(), &HookChain::default(), "payment", &pkey, "").ok(); // payment may have no workflow
    // payment is submittable; approve it to docstatus 1 so its counter fires
    broker.db.sql_root(&format!("UPDATE payment:{pkey} SET docstatus = 1;")).unwrap();

    let ar2 = broker.db.sql_root("SELECT <string>charged AS charged, <string>paid AS paid FROM ar_outstanding:acme;").unwrap();
    let ar2 = &ar2.as_array().unwrap()[0];
    println!("AR after payment: charged={} paid={}", ar2["charged"], ar2["paid"]);
    assert_eq!(ar2["paid"].as_str(), Some("20.98"), "AR paid = payment amount, exact");

    // outstanding = charged - paid = 50.98 - 20.98 = 30.00, computed exact.
    // Assert the VALUE, not its representation (30.00 == 30) — ask the DB.
    let exact = broker.db.sql_root(
        "SELECT VALUE (charged - paid) = 30.00dec FROM ar_outstanding:acme;").unwrap();
    let exact = exact.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null);
    let shown = broker.db.sql_root(
        "SELECT VALUE <string>(charged - paid) FROM ar_outstanding:acme;").unwrap();
    println!("outstanding = {}", shown.as_array().and_then(|a| a.first()).and_then(|v| v.as_str()).unwrap_or_default());
    assert_eq!(exact, serde_json::json!(true), "outstanding = 50.98 - 20.98 = EXACTLY 30.00");

    // ── cancel-reversal: cancelling the payment reverses its AR contribution ──
    // docstatus 1 → 2 is the cancel; the counter's signed algebra (WO-007)
    // subtracts $before back out.
    broker.db.sql_root(&format!("UPDATE payment:{pkey} SET docstatus = 2;")).unwrap();
    let ar3 = broker.db.sql_root("SELECT <string>paid AS paid FROM ar_outstanding:acme;").unwrap();
    println!("AR after payment cancel: paid={}", ar3.as_array().unwrap()[0]["paid"]);
    let reversed = broker.db.sql_root(
        "SELECT VALUE paid = 0dec FROM ar_outstanding:acme;").unwrap();
    assert_eq!(
        reversed.as_array().and_then(|a| a.first()).cloned().unwrap_or(serde_json::Value::Null),
        serde_json::json!(true),
        "cancelling the payment reverses paid back to exactly 0"
    );

    // ── 5. uninstall is honest: metadata detaches, data remains ──
    let un = call(&rest, "/app/acct/uninstall", &mgr(), serde_json::json!({})).expect("uninstall");
    println!("uninstalled: {un}");
    assert_eq!(un["action"], serde_json::json!("uninstalled"));
    // the invoice ROW survives (data remains); its doctype metadata is gone
    let rows = broker.db.sql_root("SELECT count() AS n FROM sales_invoice GROUP ALL;").unwrap();
    let n = rows.as_array().and_then(|a| a.first()).and_then(|r| r["n"].as_u64()).unwrap_or(0);
    assert!(n >= 1, "invoice data survives uninstall");
    let dt = broker.db.sql_root("SELECT name FROM doctype WHERE name = 'sales_invoice';").unwrap();
    assert!(dt.as_array().map(|a| a.is_empty()).unwrap_or(true), "invoice metadata detached");
}
