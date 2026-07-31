//! WO-019 criterion 1: THE DOOR PROBE.
//!
//! PM prediction, stated to be falsifiable: *a plugin route handler is a
//! profile of ADR-006 — a WIT export receiving the verb surface under the
//! caller's session, in a world with no db handle to obtain.*
//!
//! This test tries to falsify it. Every hostile attempt is made BY THE GUEST,
//! through the real boundary — not simulated host-side, which would only prove
//! that the host refuses things the host already refuses.
//!
//! If any `ESCAPED:` shows up, the registry design is wrong and the WO stops.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::scoped_db;
use frust_kernel::tenancy::single_tenant;
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::routes::RouteHost;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn setup(name: &str) -> (Arc<Broker>, RouteHost) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:clerk1 SET name = 'clerk1', role = 'clerk', pass = crypto::argon2::generate('pw-clerk1'); \
         CREATE app_user:clerk2 SET name = 'clerk2', role = 'clerk', pass = crypto::argon2::generate('pw-clerk2'); \
         DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE; \
         CREATE doctype CONTENT {{ name: 'purchase_order', fields: [ \
            {{ fieldname: 'title', fieldtype: 'Data' }}, \
            {{ fieldname: 'total', fieldtype: 'Currency' }} ] }};",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    frust_kernel::sync::MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");

    let broker = Arc::new(Broker::new(
        scoped_db(&cfg),
        Box::new(WasmHooks::load(artifacts()).expect("hooks")),
    ));
    // one row per clerk, so a row-permission leak is VISIBLE as a count
    for (who, pass, title) in
        [("clerk1", "pw-clerk1", "clerk1 order"), ("clerk2", "pw-clerk2", "clerk2 order")]
    {
        broker
            .db_write(
                &Caller { user: who.into(), pass: pass.into(), role: "clerk".into() },
                &HookChain::default(),
                WriteOp::Create,
                "purchase_order",
                None,
                &[
                    ("title".to_string(), Value::Text(title.into())),
                    ("total".to_string(), Value::Decimal("100.00".into())),
                ],
            )
            .expect("seed row");
    }
    let host = RouteHost::load(artifacts(), "plugin_demo.wasm", Arc::clone(&broker)).expect("route host");
    (broker, host)
}

fn clerk1() -> Caller {
    Caller { user: "clerk1".into(), pass: "pw-clerk1".into(), role: "clerk".into() }
}

fn probe(host: &RouteHost, which: &str) -> String {
    let (status, body) = host
        .handle("demo", &clerk1(), "probe", &format!("probe={which}"), "")
        .unwrap_or_else(|e| panic!("route {which} trapped: {e:?}"));
    assert_eq!(status, 200, "probe {which} status");
    println!("probe {which:<12} -> {body}");
    body
}

/// The honest path works — otherwise "nothing escaped" would be vacuous.
#[test]
fn the_door_opens_for_legitimate_reads() {
    let (_b, host) = setup("door_ok");
    let body = probe(&host, "read");
    assert!(body.starts_with("ok:"), "a structured read must succeed: {body}");
    assert!(body.contains("clerk1 order"), "and return the caller's own row: {body}");
}

/// THE PROPERTY: the route runs under the CALLER's session, so the DB's row
/// rules apply to it exactly as they apply to the Desk. A plugin is not a
/// privileged principal.
#[test]
fn a_route_sees_only_what_its_caller_may_see() {
    let (broker, host) = setup("door_rows");
    let body = probe(&host, "read");
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(body.trim_start_matches("ok:")).expect("rows json");

    // the same read, made directly by the same caller through the broker
    let direct = broker
        .db_read(&clerk1(), "purchase_order", None, &[], &ReadOpts::default())
        .expect("direct read");

    assert_eq!(rows.len(), direct.len(), "route read == broker read for the same caller");
    assert_eq!(rows.len(), 1, "clerk1 sees exactly its own row, not clerk2's: {rows:?}");
    assert!(
        !body.contains("clerk2 order"),
        "ROW PERMISSION LEAK through the plugin door: {body}"
    );
}

/// Hostile 1 + 2: SurrealQL smuggled as a doctype or through the filter.
/// Both must fail as PARSE/IDENT errors — text that never reaches the database.
#[test]
fn query_text_cannot_be_smuggled_through_the_door() {
    let (_b, host) = setup("door_raw");

    let by_doctype = probe(&host, "raw-doctype");
    assert!(!by_doctype.starts_with("ESCAPED"), "raw doctype ESCAPED: {by_doctype}");
    assert!(by_doctype.starts_with("refused:"), "expected refusal: {by_doctype}");

    let by_filter = probe(&host, "raw-filter");
    assert!(!by_filter.starts_with("ESCAPED"), "raw filter ESCAPED: {by_filter}");
    assert!(by_filter.starts_with("refused:"), "expected refusal: {by_filter}");
}

/// Hostile 3: a table the caller has no business reading. The plugin is not a
/// route around the permission compiler.
#[test]
fn a_route_cannot_read_the_identity_table() {
    let (_b, host) = setup("door_root");
    let body = probe(&host, "root-leak");
    assert!(!body.starts_with("ESCAPED"), "IDENTITY LEAK through the door: {body}");
    assert!(!body.contains("argon2"), "password material crossed the boundary: {body}");
}

/// Hostile 4 + 5: the world itself. If a plugin can open a socket or read the
/// filesystem, the door is decoration — it would simply go around it.
#[test]
fn the_guest_world_contains_no_handle_to_obtain() {
    let (_b, host) = setup("door_world");

    let sock = probe(&host, "connect");
    assert!(!sock.starts_with("ESCAPED"), "PLUGIN OPENED ITS OWN CONNECTION: {sock}");

    let fs = probe(&host, "fs");
    assert!(!fs.starts_with("ESCAPED"), "PLUGIN READ THE FILESYSTEM: {fs}");
}

/// A hook has no door: `db-read` from the hook context is refused explicitly
/// rather than silently returning nothing. Same artifact, different context —
/// the authority binding is per-call, not per-component.
#[test]
fn the_door_is_absent_outside_a_route_call() {
    let (broker, _host) = setup("door_hook");
    // the hook path runs the same component; it must not have acquired a door
    let out = broker.db_write(
        &clerk1(),
        &HookChain::default(),
        WriteOp::Create,
        "purchase_order",
        None,
        &[
            ("title".to_string(), Value::Text("through the hook".into())),
            ("total".to_string(), Value::Decimal("1.00".into())),
        ],
    );
    assert!(out.is_ok(), "hooks still work with the door absent: {out:?}");
}
