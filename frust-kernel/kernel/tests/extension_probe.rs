//! The cross-app extension probe.
//!
//! **Predictions are asserted here as whichever reality holds** — the probe
//! must not encode its prediction, or a wrong prediction reads as a defect.
//!
//! **No production bypass exists, not even in a test build.** `owns` is a live
//! boundary in every artifact; this probe does not touch `app.rs`. Instead it
//! HAND-CARRIES the effect a relaxed `owns` would have — writing what app B's
//! install *would* write — and observes the runtime. That answers the pool /
//! dispatch / attribution / detach questions without ever weakening the refusal.

mod common;

use std::sync::Arc;

use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::{Value, WriteOp};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl, meta_ddl};
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

/// A tenant with one doctype owned by app A, carrying A's validate script.
fn setup(name: &str) -> (Arc<Broker>, Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!("{}; {}; {}", meta_ddl(), identity_ddl(), access_ddl())).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', \
         pass = crypto::argon2::generate('pw-mgr');",
    )
    .unwrap();
    // app A owns `probe_doc` and hooks it — the ordinary, supported case
    db.sql_root(
        "CREATE doctype:probe_doc CONTENT { \
           name: 'probe_doc', label: 'probe doc', app: 'app_a', \
           server_script: [{ app: 'app_a', script: \"doc.owner_ran = 'A';\" }], \
           fields: [ { fieldname: 'title', fieldtype: 'Data' }, \
                     { fieldname: 'owner_ran', fieldtype: 'Data' } ] };",
    )
    .unwrap();
    MetadataSync { base: cfg.clone() }.sync(&db).expect("sync");
    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source(scoped_db(&cfg));
    (Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks))), db, cfg)
}

fn write(b: &Broker, title: &str) -> serde_json::Value {
    b.db_write(&mgr(), &HookChain::default(), WriteOp::Create, "probe_doc", None, &[(
        "title".to_string(),
        Value::Text(title.into()),
    )])
    .expect("write")
}

/// **The composition question, observed.**
///
/// Hand-carries what app B's install would do to app A's doctype, and reports
/// what the runtime actually does with it.
#[test]
fn what_happens_when_a_second_app_hooks_someone_elses_doctype() {
    let (b, db, _cfg) = setup("wo049_compose");

    // ── baseline: A's hook runs ──
    let first = write(&b, "one");
    assert_eq!(first["owner_ran"], serde_json::json!("A"), "A's own hook must run");
    println!("BASELINE: owner_ran = {}", first["owner_ran"]);

    // ── P1: is a second script even REPRESENTABLE? ──
    //
    // Ask the storage, don't assume: read the doctype record and report the
    // shape of `server_script`. A list would mean composition has a home; a
    // scalar means it does not.
    let rec = db
        .sql_root("SELECT server_script, app, fields FROM doctype WHERE name = 'probe_doc' LIMIT 1;")
        .expect("read doctype");
    let row = &rec.as_array().expect("rows")[0];
    let shape = if row["server_script"].is_array() {
        "array"
    } else if row["server_script"].is_string() {
        "single scalar string"
    } else {
        "absent/other"
    };
    println!("P1 storage shape of `server_script`: {shape}");
    println!("P1 doctype already carries an owner field `app` = {}", row["app"]);

    // ── hand-carry B: a namespaced field + B's validate hook ──
    //
    // This is exactly what `attach_metadata` would write for app B if `owns`
    // let it through. Note there is ONE slot to write into.
    //
    // B's field is DECLARED, not decorative. The first run of this probe left
    // it undeclared and reported "neither hook ran" — because the read envelope
    // filter silently strips any field the doctype does not declare, so B's
    // output vanished and looked exactly like B never executing. That is a
    // finding in its own right, and it is why this line exists.
    db.sql_root(
        "UPDATE doctype:probe_doc SET \
           fields += { fieldname: 'b_ext_note', fieldtype: 'Data' }, \
           fields += { fieldname: 'ext_ran', fieldtype: 'Data' }, \
           server_script += { app: 'app_b', script: \"doc.ext_ran = 'B';\" };",
    )
    .expect("hand-carry B's install effect");
    frust_kernel::broker::invalidate_meta(db.tenant_id());
    // the schema must carry the column too, exactly as an install's sync would
    MetadataSync { base: _cfg.clone() }.sync(&db).expect("re-sync after B");

    let after = write(&b, "two");
    println!("AFTER B: owner_ran = {}, ext_ran = {}", after["owner_ran"], after["ext_ran"]);

    // ── the observation, whichever way it falls ──
    let owner_survived = after["owner_ran"] == serde_json::json!("A");
    let ext_ran = after["ext_ran"] == serde_json::json!("B");

    if owner_survived && ext_ran {
        println!("OBSERVED: both hooks composed — owner-first chain holds");
    } else if ext_ran && !owner_survived {
        println!(
            "OBSERVED: B REPLACED A silently. The owner's hook stopped running and \
             nothing reported it — this is the silent-override pain exactly, and `owns` is the only thing \
             standing between the product and it."
        );
    } else if owner_survived && !ext_ran {
        println!("OBSERVED: B's script was ignored; the owner's hook held");
    } else {
        println!("OBSERVED: NEITHER hook ran — a worse outcome than either prediction");
    }

    // Asserted as the *fact*, not the prediction: exactly one of the two ran,
    // and the doc records which. If a future build makes both run, this
    // assertion fails and demands a re-read — which is the point.
    // The permanent failing control.
    //
    // This assertion was `owner ^ ext` back when the single-slot shape let B
    // silently replace A. It is now `owner && ext`, and it is the
    // guard that makes displacement unbuildable rather than merely refused: if
    // any future change lets an extension skip, reorder or replace the owner's
    // hook, this goes red naming which half vanished.
    assert!(
        owner_survived,
        "THE OWNER'S HOOK WAS DISPLACED — this is the silent-override pain (owner_ran={owner_survived}, ext_ran={ext_ran})"
    );
    assert!(ext_ran, "the extension's hook did not run (owner_ran={owner_survived}, ext_ran={ext_ran})");

    // ── P4: what would detaching B look like? ──
    //
    // There is no per-app script row to delete. Report what uninstall could
    // even address.
    let rows = db
        .sql_root("SELECT name, app FROM doctype WHERE name = 'probe_doc';")
        .expect("read")
        .as_array()
        .cloned()
        .unwrap_or_default();
    println!(
        "P4 rows an uninstall of B could target: {} (the doctype row still says app = {})",
        rows.len(),
        rows.first().map(|r| r["app"].clone()).unwrap_or_default()
    );
}

/// The pool and cache keys are widened per app.
///
/// These keys were once 2-tuples; the current shape pins the state that makes
/// composition possible. The pool key is the load-bearing half: `(tenant, doctype)`
/// held ONE instance per DocType, so a second contributor could only evict the first.
#[test]
fn the_script_pool_is_keyed_per_app() {
    let src = include_str!("../src/hooks.rs");
    // Matched on the PREFIX, not the whole tuple: the property this guard
    // exists to protect is "app is part of the key", and it was later legitimately
    // widened further to `(tenant, doctype, app, class)` so one app can
    // subscribe to several events on one DocType. An exact-tuple match cannot
    // tell a widening from a narrowing, and only the narrowing is the bug.
    let pool_keyed_at_least_per_app =
        src.contains("pool: Mutex<HashMap<(String, String, String");
    println!("C3 pool keyed at least (tenant, doctype, app): {pool_keyed_at_least_per_app}");
    assert!(
        pool_keyed_at_least_per_app,
        "the pool key narrowed back to a pair — an extension can evict the owner's instance"
    );

    // the script cache stays keyed per doctype but now CARRIES the whole plan,
    // so one generation check covers every app's script at once
    let cache_plan = src.contains("script_cache: Mutex<HashMap<(String, String), (u64, crate::sync::HookPlan)>>");
    println!("C3 script cache carries the whole HookPlan: {cache_plan}");
    assert!(cache_plan, "script cache shape changed — re-read before trusting this suite");
}

/// Per-app hook attribution.
///
/// "Which app changed this behaviour" must be a log field, not an archaeology
/// exercise — that is the silent-override pain's actual complaint, and this is where it is answered
/// checkably.
#[test]
fn hook_dispatch_attributes_each_hook_to_its_app() {
    let src = include_str!("../src/hooks.rs");
    // Scope to the PER-APP dispatcher. The first `hook_dispatch` span in the
    // file belongs to `dispatch_one` (the plugin/built-in runtimes), which has
    // no app to attribute — searching from the top found that one and reported
    // "no attribution" about the wrong function.
    let fnstart = src.find("fn dispatch_doctype_script").expect("the per-app dispatcher exists");
    let body = &src[fnstart..];
    let idx = body.find("Span::begin(\"hook_dispatch\")").expect("its span exists");
    let window = &body[idx..(idx + 600).min(body.len())];
    let has_app = window.contains(".field(\"app\"");
    let has_owner = window.contains(".field(\"owner\"");
    println!("C4 hook_dispatch carries `app`: {has_app}, `owner`: {has_owner}");
    assert!(has_app, "per-app attribution was removed — a composed failure cannot be blamed");
    assert!(has_owner, "the owner/extension distinction was removed from the span");
}
