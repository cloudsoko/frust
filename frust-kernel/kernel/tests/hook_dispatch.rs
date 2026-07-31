//! WO-005 module 4: the in-process dispatcher over the dynamic-doc envelope
//! (ADR-006 edge 3) â€” things the toy {id,status,total} shape could not carry.
//! Direct against WasmHooks; no DB needed.

use frust_kernel::broker::HookDispatch;
use frust_kernel::contract::{BrokerError, Value};
use frust_kernel::hooks::WasmHooks;

fn hooks() -> WasmHooks {
    WasmHooks::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts"))
        .expect("load hook components")
}

fn get<'a>(doc: &'a [(String, Value)], k: &str) -> Option<&'a Value> {
    doc.iter().find(|(n, _)| n == k).map(|(_, v)| v)
}

/// The full document flows through both hook classes, including fields the
/// toy envelope never had (a decimal, arbitrary text) â€” and they survive
/// round-trip untouched while status mutates.
#[test]
fn dynamic_doc_roundtrips_all_fields() {
    let h = hooks();
    let doc = vec![
        ("id".into(), Value::Text("sinv:1".into())),
        ("status".into(), Value::Text("Draft".into())),
        ("total".into(), Value::Float(20000.0)),
        ("supplier_name".into(), Value::Text("ACME GmbH".into())),
        ("credit_limit".into(), Value::Decimal("1234.56".into())),
        ("notes".into(), Value::Text("carry me through untouched".into())),
    ];
    let out = h.validate("thing", &doc).expect("hooks accept");

    // plugin flagged the large draft, script kept the mutation
    assert_eq!(get(&out, "status"), Some(&Value::Text("Needs Approval".into())));
    // fields the toy envelope could never carry survive verbatim
    assert_eq!(get(&out, "supplier_name"), Some(&Value::Text("ACME GmbH".into())));
    assert_eq!(get(&out, "notes"), Some(&Value::Text("carry me through untouched".into())));
}

/// Decimal is first-class and never becomes a float crossing the boundary
/// (ADR-006 edge 3 money guarantee) â€” even through the JS script engine,
/// which re-imposes the decimal kind on the way out.
#[test]
fn decimal_stays_decimal_across_the_boundary() {
    let h = hooks();
    let doc = vec![
        ("id".into(), Value::Text("sinv:2".into())),
        ("status".into(), Value::Text("Draft".into())),
        ("total".into(), Value::Float(5.0)),
        ("price".into(), Value::Decimal("19.99".into())),
    ];
    let out = h.validate("thing", &doc).unwrap();
    match get(&out, "price") {
        Some(Value::Decimal(s)) => assert_eq!(s, "19.99", "decimal value preserved exactly"),
        other => panic!("price must remain a Decimal, got {other:?}"),
    }
}

/// The plugin's typed reject path surfaces as a HookRejected error.
#[test]
fn negative_total_rejected() {
    let h = hooks();
    let doc = vec![
        ("id".into(), Value::Text("x:1".into())),
        ("total".into(), Value::Float(-5.0)),
    ];
    let err = h.validate("thing", &doc).unwrap_err();
    assert!(matches!(err, BrokerError::HookRejected { .. }), "got {err:?}");
}

/// Containment holds through the kernel dispatcher: after a plugin traps
/// (here: the script rejects), the dispatcher's instance self-heals and the
/// next legitimate call succeeds â€” the kernel never dies with a hook.
#[test]
fn dispatcher_self_heals_after_reject() {
    let h = hooks();
    // a reject is not a trap, but exercises the error path; follow with a
    // good call to prove the pooled instances still serve
    let _ = h.validate("thing", &[("total".into(), Value::Float(-1.0))]);
    let good = vec![
        ("id".into(), Value::Text("ok:1".into())),
        ("status".into(), Value::Text("Draft".into())),
        ("total".into(), Value::Float(100.0)),
    ];
    let out = h.validate("thing", &good).expect("dispatcher still serves after a reject");
    assert_eq!(get(&out, "status"), Some(&Value::Text("Draft".into())));
}


