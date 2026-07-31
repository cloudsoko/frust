//! WO-019 criterion 2: the App manifest — one file format, validated before
//! anything applies, with REQ-6.6's gate discipline extending to bundles.
//!
//! Requires surreal.exe on :8899 (root/root), ns frust.

use frust_kernel::app::{version_gt, Manifest, MANIFEST_VERSION};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};
use frust_kernel::meta::{access_ddl, identity_ddl};

fn setup(name: &str) -> (Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!(
        "{}; {}; DEFINE TABLE doctype SCHEMALESS PERMISSIONS NONE;",
        identity_ddl(),
        access_ddl()
    ))
    .unwrap();
    (db, cfg)
}

fn good_bundle() -> String {
    serde_json::json!({
        "manifest_version": MANIFEST_VERSION,
        "name": "acct",
        "version": "1.0.0",
        "label": "Accounting",
        "doctypes": [{
            "name": "acct_invoice",
            "app": "acct",
            "submittable": true,
            "fields": [
                { "fieldname": "customer", "fieldtype": "Data", "required": true },
                { "fieldname": "total", "fieldtype": "Currency" }
            ]
        }],
        "client_scripts": [{ "doctype": "acct_invoice", "script": "doc.customer = doc.customer;" }],
        "server_scripts": [{ "doctype": "acct_invoice", "hook": "validate", "script": "doc;" }],
        "routes": [{ "path": "ledger", "component": "plugin_demo.wasm" }],
        "components": ["plugin_demo.wasm"],
        // RESERVED — WO-018 designs this; here it must merely survive
        // WO-018 filled this slot: it is now a typed WorkflowDef, validated
        // like everything else, so the fixture is a real (minimal) workflow.
        "workflows": [{
            "name": "approval",
            "doctype": "acct_invoice",
            "states": [
                { "name": "Draft", "docstatus": 0 },
                { "name": "Approved", "docstatus": 1 }
            ],
            "transitions": [
                { "from": "Draft", "to": "Approved", "role": "manager", "action": "Approve" }
            ]
        }]
    })
    .to_string()
}

/// One file format that round-trips — including the slot this WO deliberately
/// does not design.
#[test]
fn a_bundle_round_trips_and_preserves_the_reserved_workflow_slot() {
    let m = Manifest::parse(&good_bundle()).expect("parses");
    assert!(m.validate().is_empty(), "valid bundle: {:?}", m.validate());
    assert_eq!(m.workflows.len(), 1, "workflow slot carried");

    let out = serde_json::to_string(&m).expect("serializes");
    let again = Manifest::parse(&out).expect("re-parses");
    // WorkflowDef is not PartialEq (it is a metadata shape, not a value type),
    // so compare the round trip through JSON — which is what "survives a round
    // trip verbatim" actually means for a serialized bundle.
    assert_eq!(
        serde_json::to_value(&again.workflows).unwrap(),
        serde_json::to_value(&m.workflows).unwrap(),
        "reserved slot survives a round trip verbatim"
    );
    assert_eq!(again.doctypes.len(), 1);
    assert_eq!(again.doctypes[0].fields.len(), 2);
    assert_eq!(again.routes[0].path, "ledger");
    assert!(again.doctypes[0].submittable, "submittable survives");
}

/// Validation reports EVERY problem, the way `MigrationReport::errors` does.
/// Peeling one error per attempt is its own kind of hostility.
#[test]
fn validation_collects_every_problem_at_once() {
    let bad = serde_json::json!({
        "manifest_version": 99,
        "name": "9bad name",
        "version": "1.0",
        "doctypes": [
            { "name": "ok_one", "fields": [{ "fieldname": "bad field", "fieldtype": "Data" }] },
            { "name": "ok_one", "fields": [] }
        ],
        "client_scripts": [{ "doctype": "not_in_bundle", "script": "x;" }],
        "server_scripts": [{ "doctype": "ok_one", "hook": "before_save", "script": "x;" }],
        "routes": [{ "path": "fine", "component": "missing.wasm" }],
        "components": ["../../etc/passwd.wasm"]
    })
    .to_string();

    let errs = Manifest::parse(&bad).expect("still parses structurally").validate();
    for e in &errs {
        println!("error: {e}");
    }
    let joined = errs.join("\n");
    assert!(joined.contains("manifest_version"), "{joined}");
    assert!(joined.contains("not an identifier"), "{joined}");
    assert!(joined.contains("MAJOR.MINOR.PATCH"), "{joined}");
    assert!(joined.contains("declared twice"), "{joined}");
    assert!(joined.contains("not_in_bundle"), "{joined}");
    assert!(joined.contains("only 'validate' exists today"), "{joined}");
    assert!(joined.contains("missing.wasm"), "{joined}");
    assert!(joined.contains("bare .wasm filename"), "{joined}");
    assert!(errs.len() >= 8, "expected the whole list, got {}: {joined}", errs.len());
}

/// Nothing applies before validation passes — the refusal names the problems
/// rather than half-installing and reporting a failure afterwards.
#[test]
fn an_invalid_bundle_never_reaches_the_migrator() {
    let (db, cfg) = setup("app_invalid");
    let bad = serde_json::json!({
        "manifest_version": MANIFEST_VERSION,
        "name": "broken",
        "version": "nope",
        "doctypes": [{ "name": "b_thing", "fields": [] }]
    })
    .to_string();
    let m = Manifest::parse(&bad).unwrap();
    let err = m.plan(&cfg, &db).expect_err("must refuse");
    println!("refused: {err:?}");
    assert!(format!("{err:?}").contains("not installable"));

    // and the schema is untouched
    let tables = db.sql_root("INFO FOR DB;").unwrap();
    assert!(
        !tables.to_string().contains("b_thing"),
        "an invalid bundle must not have created anything"
    );
}

/// REQ-6.6.1 becomes UX: the plan shows the DDL that WOULD run, takes no lock,
/// and leaves the schema untouched. Crucially it is the sync engine's own
/// dry-run — not a second migration path built for bundles.
#[test]
fn dry_run_previews_the_bundle_without_touching_schema() {
    let (db, cfg) = setup("app_plan");
    let m = Manifest::parse(&good_bundle()).unwrap();
    let plan = m.plan(&cfg, &db).expect("plan");

    println!(
        "plan: app={} v{} planned={} applied={} routes={:?} workflows={}",
        plan.app,
        plan.version,
        plan.schema.planned.len(),
        plan.schema.applied.len(),
        plan.routes,
        plan.workflows
    );
    assert!(!plan.schema.planned.is_empty(), "a dry run yields a plan");
    assert!(plan.schema.applied.is_empty(), "a dry run applies nothing");
    let ddl = format!("{:?}", plan.schema.planned);
    assert!(ddl.contains("acct_invoice"), "the plan names the table: {ddl}");

    // the whole truth, not just the DDL half
    assert_eq!(plan.routes, vec!["/app/acct/ledger".to_string()]);
    assert_eq!(plan.client_scripts, vec!["acct_invoice".to_string()]);
    assert_eq!(plan.workflows, 1);

    // schema genuinely untouched
    let info = db.sql_root("INFO FOR DB;").unwrap();
    assert!(!info.to_string().contains("acct_invoice"), "dry run must not create the table");
}

/// The gate travels with the bundle: applying the same manifest for real does
/// create the table, and the plan/apply call shape is identical — which is what
/// stops "what you were shown" from drifting away from "what ran".
#[test]
fn the_same_call_shape_applies_when_told_to() {
    use frust_orm::MigrationOptions;
    let (db, cfg) = setup("app_apply");
    let m = Manifest::parse(&good_bundle()).unwrap();

    let applied = m
        .plan_unchecked(&cfg, &db, MigrationOptions::default_for_dev())
        .expect("apply");
    assert!(!applied.schema.applied.is_empty(), "a real run applies");
    assert!(applied.schema.planned.is_empty(), "a real run has no plan half");

    let info = db.sql_root("INFO FOR DB;").unwrap();
    assert!(info.to_string().contains("acct_invoice"), "the table exists now");
}

/// Update detection for criterion 3 — versions order, and malformed ones say
/// so instead of comparing as strings ("10.0.0" < "9.0.0" lexically).
#[test]
fn versions_order_numerically_not_lexically() {
    assert_eq!(version_gt("10.0.0", "9.0.0"), Some(true), "numeric, not lexical");
    assert_eq!(version_gt("1.2.3", "1.2.3"), Some(false));
    assert_eq!(version_gt("1.3.0", "1.2.9"), Some(true));
    assert_eq!(version_gt("1.0", "1.0.0"), None, "malformed is None, never a silent false");
}
