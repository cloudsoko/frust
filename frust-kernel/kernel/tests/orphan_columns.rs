//! Orphan columns: carrying a column the metadata no longer declares.
//!
//! **This regression is the point of the file, and it comes first.** Earlier
//! lifecycle tests exercised install, exercise, uninstall and re-install — every
//! lifecycle step *within one kernel lifetime* — and never restarted the kernel
//! after an uninstall.
//! The defect lived exactly one process boundary past the suite's horizon: the
//! uninstall left a column the metadata no longer declared, and the next boot's
//! sync classified it destructive and refused to start, with no operator
//! remedy.
//!
//! So the regression here does not "test the fix". It reproduces the shipped
//! blocker, and it was watched failing on `E_BOOT_DB` before the fix existed.

mod common;

use std::sync::Arc;

use frust_kernel::boot::{boot, BootOptions};
use frust_kernel::broker::{Broker, Caller, HookChain};
use frust_kernel::contract::{BrokerError, Value, WriteOp};
use frust_kernel::db::{scoped_db, Db};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::meta::{access_ddl, identity_ddl, meta_ddl};
use frust_kernel::rest::Rest;
use frust_kernel::sync::MetadataSync;
use frust_kernel::tenancy::{single_tenant, ResolvedTenant};

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}
fn mgr() -> Caller {
    Caller { user: "mgr".into(), pass: "pw-mgr".into(), role: "manager".into() }
}

fn setup(name: &str) -> (Rest, Db, ResolvedTenant) {
    let cfg = single_tenant(&name.to_string()).expect("tenancy");
    let db = scoped_db(&cfg);
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};")).unwrap();
    db.sql_root(&format!("{}; {}; {}", meta_ddl(), identity_ddl(), access_ddl())).unwrap();
    db.sql_root(
        "CREATE app_user:mgr SET name = 'mgr', role = 'manager', \
         pass = crypto::argon2::generate('pw-mgr');",
    )
    .unwrap();
    let hooks = WasmHooks::load(artifacts()).expect("hooks").with_script_source(scoped_db(&cfg));
    let broker = Arc::new(Broker::new(scoped_db(&cfg), Box::new(hooks)));
    let rest = Rest::single(
        broker,
        "127.0.0.1:0".into(),
        Some(Arc::new(MetadataSync { base: cfg.clone() })),
        None,
    );
    (rest, db, cfg)
}

fn call(rest: &Rest, path: &str, body: serde_json::Value) -> Result<serde_json::Value, BrokerError> {
    rest.route_for_test(path, &body, &mgr())
}

fn owner_app() -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1, "name": "acct", "version": "1.0.0",
        "doctypes": [{ "name": "inv", "fields": [
            { "fieldname": "title", "fieldtype": "Data" },
            { "fieldname": "owner_ran", "fieldtype": "Data" }
        ]}],
        "server_scripts": [{ "doctype": "inv", "hook": "validate",
                             "script": "doc.owner_ran = 'A';" }]
    })
}

fn ext_app() -> serde_json::Value {
    serde_json::json!({
        "manifest_version": 1, "name": "crm", "version": "1.0.0",
        "extends": [{ "doctype": "inv", "hook": "validate",
                      "fields": [{ "fieldname": "crm_tag", "fieldtype": "Data" }],
                      "script": "doc.crm_tag = 'KEEPME';" }]
    })
}

fn broker_for(cfg: &ResolvedTenant) -> Broker {
    let hooks = WasmHooks::load(artifacts()).unwrap().with_script_source(scoped_db(cfg));
    Broker::new(scoped_db(cfg), Box::new(hooks))
}

/// A kernel restart, as faithfully as a test can stage one: the SAME boot the
/// binary runs, against the same store, with a fresh sync engine.
fn restart(cfg: &ResolvedTenant) -> Result<frust_kernel::boot::BootReport, frust_kernel::boot::BootError> {
    let db = scoped_db(cfg);
    let sync = MetadataSync { base: cfg.clone() };
    boot(&db, &BootOptions::default(), &sync)
}

/// **Criterion 5's seam, read the way an operator would read it.** The boot
/// report is a log line; `/metrics` is the thing you can poll. Returns the
/// gauge's value, or `None` if the series does not exist at all.
fn orphan_gauge(tenant: &str, column: &str) -> Option<f64> {
    let text = frust_kernel::telemetry::render_prometheus();
    let want = format!("column=\"{column}\"");
    let who = format!("tenant=\"{tenant}\"");
    text.lines()
        .find(|l| l.starts_with("frust_orphan_column{") && l.contains(&want) && l.contains(&who))
        .and_then(|l| l.rsplit(' ').next())
        .and_then(|v| v.parse().ok())
}

// ── criterion 2: the regression whose absence shipped the blocker ──────────

/// **install extension → exercise → uninstall → RESTART.**
///
/// Against pre-fix behaviour this failed with
/// `E_BOOT_DB: refusing destructive change(s) ["REMOVE FIELD crm_tag"]`,
/// which is the exact production symptom seen bringing the dev stack back
/// up. It is the one step the earlier lifecycle suite never took.
#[test]
fn the_kernel_restarts_after_an_extension_uninstall() {
    let (rest, _db, cfg) = setup("wo052_restart");
    call(&rest, "/app/install", owner_app()).expect("owner");
    call(&rest, "/app/install", ext_app()).expect("extension");

    {
        let b = broker_for(&cfg);
        let row = b
            .db_write(&mgr(), &HookChain::default(), WriteOp::Create, "inv", None, &[(
                "title".to_string(),
                Value::Text("one".into()),
            )])
            .expect("write");
        assert_eq!(row["crm_tag"], serde_json::json!("KEEPME"), "the extension ran");
    }

    call(&rest, "/app/crm/uninstall", serde_json::json!({})).expect("uninstall the extension");

    // THE STEP NOBODY TOOK
    let report = restart(&cfg).expect(
        "the kernel MUST boot after an extension uninstall — a data-safety guard \
         that refuses to start has become an availability outage",
    );

    // criterion 1 + 5: the orphan is NAMED, not merely tolerated
    println!("boot report orphans: {:?}", report.orphan_columns);
    assert!(
        report.orphan_columns.iter().any(|o| o.contains("crm_tag")),
        "the orphan must be NAMED in the boot report — an orphan nobody can list is \
         the silent-wrong shape in ops clothing: {:?}",
        report.orphan_columns
    );

    // criterion 5: and it is enumerable WITHOUT reading a log — /metrics names
    // it, so "which columns are orphaned here" is a query, not an archaeology
    assert_eq!(
        orphan_gauge("wo052_restart", "inv.crm_tag"),
        Some(1.0),
        "the orphan must be visible on /metrics:
{}",
        frust_kernel::telemetry::render_prometheus()
    );

    // and boot did not quietly drop it
    let cols = scoped_db(&cfg)
        .sql_root("SELECT title, crm_tag FROM inv;")
        .expect("the column must still exist after boot");
    println!("rows after restart: {cols}");
}

// ── criterion 3: re-adoption, asserted BY VALUE ────────────────────────────

/// A count cannot prove data came back. This asserts the *datum*: the value
/// written before the uninstall is readable again after the extension is
/// re-installed, across a restart.
#[test]
fn reinstalling_after_a_restart_re_adopts_the_column_and_its_data() {
    let (rest, db, cfg) = setup("wo052_readopt");
    call(&rest, "/app/install", owner_app()).expect("owner");
    call(&rest, "/app/install", ext_app()).expect("extension");

    // a datum that must survive the whole cycle, distinguishable from anything
    // a re-install could recreate
    db.sql_root("UPDATE inv CONTENT { title: 'keeper', crm_tag: 'ORIGINAL-VALUE' };").ok();
    let created = db
        .sql_root("CREATE inv SET title = 'keeper', crm_tag = 'ORIGINAL-VALUE';")
        .expect("seed a row with the extension's data");
    println!("seeded: {created}");

    call(&rest, "/app/crm/uninstall", serde_json::json!({})).expect("uninstall");
    restart(&cfg).expect("boots with the orphan");

    // the data is still there while ORPHANED — that is the promise
    let mid = db.sql_root("SELECT crm_tag FROM inv WHERE title = 'keeper';").expect("read");
    let mid_val = mid.as_array().and_then(|a| a.first()).map(|r| r["crm_tag"].clone());
    assert_eq!(
        mid_val,
        Some(serde_json::json!("ORIGINAL-VALUE")),
        "an orphaned column keeps its data — 'metadata detaches, data remains'"
    );

    // re-install: the column is re-declared and re-adopted WITH its data
    call(&rest, "/app/install", ext_app()).expect("re-install the extension");
    let after = db.sql_root("SELECT crm_tag FROM inv WHERE title = 'keeper';").expect("read");
    let val = after.as_array().and_then(|a| a.first()).map(|r| r["crm_tag"].clone());
    assert_eq!(
        val,
        Some(serde_json::json!("ORIGINAL-VALUE")),
        "re-install must RE-ADOPT the data, not just re-create an empty column"
    );
}

// ── criterion 1: meta stays fail-closed ────────────────────────────────────

/// Orphan tolerance is for USER doctypes. The meta discipline is untouched: a
/// database claiming a newer meta version than the binary still refuses.
#[test]
fn meta_remains_fail_closed() {
    let (_rest, db, cfg) = setup("wo052_meta");
    db.sql_root(&format!(
        "UPSERT {}:schema SET version = {};",
        frust_kernel::meta::META_TABLE,
        frust_kernel::meta::META_SCHEMA_VERSION + 1
    ))
    .expect("claim a newer meta version");

    let err = restart(&cfg).expect_err("a newer-than-binary meta version must still refuse boot");
    println!("meta refusal: {err:?}");
    assert!(
        format!("{err:?}").to_lowercase().contains("meta"),
        "the refusal must still be the meta one: {err:?}"
    );
}

// ── the orphan must not FREEZE the DocType ─────────────────────────────────

/// **Tolerating the refusal is not enough — the column has to be carried.**
///
/// The migrator abandons a whole resource when its diff is refused, so a
/// kernel that merely classified the refusal as "drift" and booted anyway
/// would leave that DocType unable to take *any* further schema change: the
/// owner's next release would add a field, the sync would skip the resource,
/// and the only trace would be an orphan line that was already there. Green
/// boot, silent freeze — the exact shape this project exists to refuse.
///
/// So: an orphaned DocType keeps evolving, and the orphan keeps its data.
#[test]
fn a_doctype_with_an_orphan_still_takes_new_schema() {
    let (rest, db, cfg) = setup("wo052_evolve");
    call(&rest, "/app/install", owner_app()).expect("owner");
    call(&rest, "/app/install", ext_app()).expect("extension");
    db.sql_root("CREATE inv SET title = 'keeper', crm_tag = 'KEEP';").expect("seed");
    call(&rest, "/app/crm/uninstall", serde_json::json!({})).expect("uninstall");
    let report = restart(&cfg).expect("boots with the orphan");
    assert!(report.orphan_columns.iter().any(|o| o.contains("crm_tag")), "orphan present");

    // the owner ships its next release, adding a field, through the real door
    let mut v2 = owner_app();
    v2["version"] = serde_json::json!("1.1.0");
    v2["doctypes"][0]["fields"] = serde_json::json!([
        { "fieldname": "title", "fieldtype": "Data" },
        { "fieldname": "owner_ran", "fieldtype": "Data" },
        { "fieldname": "memo", "fieldtype": "Data" }
    ]);
    let out = call(&rest, "/app/update", v2).expect(
        "an orphan must not freeze its DocType — the owner's next release has to land",
    );
    println!("owner update over an orphan: {out}");

    // the new column is REAL, not merely planned
    let info = db.sql_root("INFO FOR TABLE inv;").expect("info");
    let fields = info["fields"].as_object().expect("fields");
    assert!(fields.contains_key("memo"), "the added field must exist in the schema: {fields:?}");
    assert!(fields.contains_key("crm_tag"), "and the orphan must still be there: {fields:?}");

    // usable, and the orphan's data untouched
    let b = broker_for(&cfg);
    b.db_write(&mgr(), &HookChain::default(), WriteOp::Create, "inv", None, &[(
        "memo".to_string(),
        Value::Text("written through the new column".into()),
    )])
    .expect("the new field takes a write");
    let kept = db.sql_root("SELECT crm_tag FROM inv WHERE title = 'keeper';").expect("read");
    assert_eq!(
        kept.as_array().and_then(|a| a.first()).map(|r| r["crm_tag"].clone()),
        Some(serde_json::json!("KEEP")),
        "the orphan keeps its data across the owner's update"
    );

    // and it is still an orphan afterwards — carrying it is not adopting it
    let after = restart(&cfg).expect("boots");
    assert!(
        after.orphan_columns.iter().any(|o| o.contains("crm_tag")),
        "still an orphan after the update: {:?}",
        after.orphan_columns
    );
}

// ── criterion 4: reclaim is an explicit, acknowledged act ──────────────────

/// Dropping an orphan is possible, but never casual: the refusal names the
/// column and the rows that still hold data, and only an explicit
/// acknowledgment applies it. No boot flag exists to do this by accident.
#[test]
fn reclaiming_an_orphan_requires_an_explicit_acknowledgment() {
    let (rest, db, cfg) = setup("wo052_reclaim");
    call(&rest, "/app/install", owner_app()).expect("owner");
    call(&rest, "/app/install", ext_app()).expect("extension");
    db.sql_root("CREATE inv SET title = 'has-data', crm_tag = 'VALUE';").expect("seed");
    call(&rest, "/app/crm/uninstall", serde_json::json!({})).expect("uninstall");
    restart(&cfg).expect("boots with the orphan");

    // a DECLARED field is not an orphan and cannot be reclaimed this way
    let err = call(&rest, "/doctype/inv/reclaim", serde_json::json!({ "column": "title", "acknowledge": true }))
        .expect_err("a declared field must not be reclaimable as an orphan");
    assert!(format!("{err:?}").contains("DECLARED"), "{err:?}");

    // the orphan, unacknowledged: refused, NAMING the column and its data
    let err = call(&rest, "/doctype/inv/reclaim", serde_json::json!({ "column": "crm_tag" }))
        .expect_err("must refuse without acknowledgment");
    let msg = format!("{err:?}");
    println!("reclaim refusal: {msg}");
    assert!(msg.contains("crm_tag"), "names the column: {msg}");
    assert!(msg.contains("row"), "names the data at stake: {msg}");

    // acknowledged: applied
    let out = call(&rest, "/doctype/inv/reclaim", serde_json::json!({ "column": "crm_tag", "acknowledge": true }))
        .expect("acknowledged reclaim applies");
    println!("reclaimed: {out}");
    assert_eq!(out["reclaimed"], serde_json::json!(true));

    // /metrics must stop naming it — a gauge map never forgets a key, so a
    // reclaimed column that keeps reporting 1 sends operators hunting a column
    // that no longer exists
    assert_eq!(
        orphan_gauge("wo052_reclaim", "inv.crm_tag"),
        Some(0.0),
        "the reclaimed column must read 0 on /metrics:
{}",
        frust_kernel::telemetry::render_prometheus()
    );

    // and the orphan is gone from the next boot's report
    let report = restart(&cfg).expect("boots");
    println!("orphans after reclaim: {:?}", report.orphan_columns);
    assert!(
        !report.orphan_columns.iter().any(|o| o.contains("crm_tag")),
        "the reclaimed orphan must no longer be reported: {:?}",
        report.orphan_columns
    );
}

/// **No boot flag was added.** Drift is treated as an orphan, so boot never
/// faces an unacknowledged destructive plan — and an `allow_destructive` startup
/// switch would be a footgun.
#[test]
fn boot_options_gained_no_destructive_flag() {
    let src = include_str!("../src/boot.rs");
    let idx = src.find("pub struct BootOptions").expect("BootOptions exists");
    let body = &src[idx..idx + 220];
    println!("BootOptions:
{body}");
    assert!(
        !body.to_lowercase().contains("destructive"),
        "a destructive boot flag was added — that is the footgun the amendment refused: {body}"
    );
}
