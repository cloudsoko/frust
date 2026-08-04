//! **Invalidation is the work, not the caching.**
//!
//! The DocType metadata cache removes a DB round trip from every request. A
//! cache that serves stale metadata after a sync or an app update would be a
//! silent-wrong bug — and it would do it in the path that decides field
//! permissions. So these tests exist to prove the cache CANNOT go stale
//! through any kernel-mediated mutation.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::*;
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn setup(name: &str) -> (Rest, Arc<Broker>, Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    // full meta DDL: the app tests need the `installed_app` registry table
    db.sql_root(&frust_kernel::meta::meta_ddl()).unwrap();
    db.sql_root(&format!(
        "{}; {}; \
         CREATE app_user:mgr SET name = 'mgr', role = 'manager', pass = crypto::argon2::generate('pw-mgr');",
        identity_ddl(),
        access_ddl(),
    ))
    .unwrap();
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(WasmHooks::load(artifacts()).unwrap())));
    let rest = Rest::single(Arc::clone(&broker), "127.0.0.1:0".into(), Some(Arc::new(MetadataSync { base: cfg.clone() })), None);
    (rest, broker, db, cfg)
}

fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

fn write(b: &Broker, dt: &str, field: &str, val: &str) -> Result<serde_json::Value, BrokerError> {
    b.db_write(&mgr(), &HookChain::default(), WriteOp::Create, dt, None,
        &[(field.to_string(), Value::Text(val.into()))])
}

/// The cache must be INVISIBLE to correctness: a doctype created through the
/// REST surface is usable immediately, with no stale "unknown doctype".
#[test]
fn a_new_doctype_is_visible_immediately() {
    let (rest, broker, _db, _cfg) = setup("mc_create");

    // prime the cache with a MISS for a doctype that does not exist yet
    let miss = write(&broker, "mc_thing", "title", "x").expect_err("not created yet");
    assert!(matches!(miss, BrokerError::UnknownDoctype { .. }), "{miss:?}");

    // create it through the surface that bumps the generation
    rest.route_for_test("/doctype", &serde_json::json!({ "meta": {
        "name": "mc_thing", "label": "T",
        "fields": [{ "fieldname": "title", "fieldtype": "Data" }] } }), &mgr())
        .expect("create doctype");

    // the very next write must see it â€” a stale negative would be the bug
    write(&broker, "mc_thing", "title", "now visible").expect("new doctype visible immediately");
}

/// A schema sync can change ANY doctype â€” the cache must not survive it.
#[test]
fn a_sync_invalidates_cached_metadata() {
    let (_rest, broker, db, cfg) = setup("mc_sync");
    db.sql_root(
        "CREATE doctype CONTENT { name: 'mc_doc', fields: [ \
           { fieldname: 'title', fieldtype: 'Data' } ] };",
    )
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync 1");

    // prime the cache: one field
    write(&broker, "mc_doc", "title", "first").expect("write with one field");

    // add a field to the metadata and re-sync (the generation bump lives in sync)
    db.sql_root(
        "UPDATE doctype SET fields = [ \
           { fieldname: 'title', fieldtype: 'Data' }, \
           { fieldname: 'memo', fieldtype: 'Data' } ] WHERE name = 'mc_doc';",
    )
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync 2");

    // the NEW field must be writable â€” a stale cache would reject it as
    // outside the doctype's field set
    write(&broker, "mc_doc", "memo", "second").expect("new field visible after sync");
}

/// App install/update writes doctype metadata through `attach_metadata`; the
/// bump there is what keeps an installed app's doctypes usable at once.
#[test]
fn an_app_install_invalidates_cached_metadata() {
    let (rest, broker, _db, _cfg) = setup("mc_app");

    // cache a miss first
    assert!(write(&broker, "mc_billed", "title", "x").is_err(), "not installed yet");

    rest.route_for_test("/app/install", &serde_json::json!({
        "manifest_version": 1, "name": "mcapp", "version": "1.0.0",
        "doctypes": [{ "name": "mc_billed", "fields": [
            { "fieldname": "title", "fieldtype": "Data" } ] }]
    }), &mgr())
    .expect("install");

    write(&broker, "mc_billed", "title", "installed").expect("app doctype usable immediately");
}

/// Uninstall detaches metadata â€” the cache must not keep the doctype alive.
#[test]
fn an_uninstall_invalidates_cached_metadata() {
    let (rest, broker, _db, _cfg) = setup("mc_uninstall");
    rest.route_for_test("/app/install", &serde_json::json!({
        "manifest_version": 1, "name": "mcapp2", "version": "1.0.0",
        "doctypes": [{ "name": "mc_gone", "fields": [
            { "fieldname": "title", "fieldtype": "Data" } ] }]
    }), &mgr())
    .expect("install");

    // prime the cache with a HIT
    write(&broker, "mc_gone", "title", "alive").expect("usable while installed");

    rest.route_for_test("/app/mcapp2/uninstall", &serde_json::json!({}), &mgr())
        .expect("uninstall");

    // a stale POSITIVE is the dangerous direction: the doctype's metadata is
    // detached, so the kernel must refuse rather than serve from cache.
    let err = write(&broker, "mc_gone", "title", "after uninstall")
        .expect_err("detached metadata must not be served from cache");
    assert!(matches!(err, BrokerError::UnknownDoctype { .. }), "{err:?}");
}

/// The cache must not leak ACROSS doctypes â€” a hit on one must never answer
/// for another (the field envelope depends on getting the right one).
#[test]
fn the_cache_is_keyed_per_doctype() {
    let (_rest, broker, db, cfg) = setup("mc_keyed");
    db.sql_root(
        "CREATE doctype CONTENT { name: 'mc_a', fields: [ { fieldname: 'aa', fieldtype: 'Data' } ] }; \
         CREATE doctype CONTENT { name: 'mc_b', fields: [ { fieldname: 'bb', fieldtype: 'Data' } ] };",
    )
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");

    write(&broker, "mc_a", "aa", "one").expect("a accepts its own field");
    write(&broker, "mc_b", "bb", "two").expect("b accepts its own field");
    // and a field belonging to the OTHER doctype is refused on read projection
    let err = broker
        .db_read(&mgr(), "mc_a", None, &[vec![PathSegment::Field("bb".into())]], &ReadOpts::default())
        .expect_err("mc_a must not inherit mc_b's fields from a shared cache");
    assert!(matches!(err, BrokerError::FieldNotReadable { .. }), "{err:?}");
}
