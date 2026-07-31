//! **WO-040 Chunk C: per-tenant restore, executed — the P-8.1 evidence.**
//!
//! WO-027 measured the cost of a single shared database: `surreal export` is
//! per-database, so restore-one *was* restore-all, and P-8.1 was scored
//! **bounded by architecture**. WO-039 proved database-per-tenant lifts it.
//! This closes it for the namespace topologies too, and it does so by
//! **running the ops path**, not by asserting a plan.
//!
//! The plan comes from the strategy (`backup_plan` → `--ns` / `--db`), the
//! export and import are the real `surreal` CLI, and the assertions are
//! provenance: *whose* rows came back, and did anyone else move.
//!
//! Requires the dev surreal on :8899 and `surreal.exe` at ../frust-skel/.

use std::process::Command;

use frust_kernel::db::{scoped_db, ConnConfig};
use frust_kernel::keyguard;
use frust_kernel::tenancy::{ResolvedTenant, Tenancy, TenancyConfig};

const SURREAL: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../frust-skel/surreal.exe");
const EP: &str = "http://127.0.0.1:8899";

fn tenancy(strategy: &str, database: Option<&str>, env: Option<&str>, tenants: &[&str]) -> Tenancy {
    Tenancy::from_config(
        ConnConfig::default(),
        TenancyConfig {
            strategy,
            namespace: None,
            database,
            environment: env,
            tenants,
        },
    )
    .expect("tenancy")
}

/// Provision through the strategy's own plan, then put one identifiable row in.
fn seed(target: &ResolvedTenant, marker: &str) {
    frust_kernel::provision::provision(target).expect("provision");
    let db = scoped_db(target);
    let dbname = target.database().as_str();
    db.sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {dbname}; DEFINE DATABASE {dbname};"))
        .expect("reset");
    db.sql_root(&format!(
        "DEFINE TABLE ledger SCHEMALESS PERMISSIONS FULL; \
         CREATE ledger SET owner_tenant = '{}', body = '{marker}';",
        target.tenant_id()
    ))
    .expect("seed row");
}

/// Every `ledger` body in this tenant's database.
///
/// A dropped database has no `ledger` table at all, and SurrealDB reports that
/// as a statement error rather than as zero rows — so that one case maps to
/// "empty" deliberately, and **every other error still panics**. Swallowing
/// all errors here would let a broken read read as "no rows", which is exactly
/// the vacuous pass this file exists to avoid.
fn bodies(target: &ResolvedTenant) -> Vec<String> {
    let stmts = scoped_db(target)
        .sql_root_raw("SELECT body FROM ledger ORDER BY body;")
        .expect("transport");
    let stmt = stmts.first().expect("one statement");
    if stmt.get("status").and_then(|s| s.as_str()) != Some("OK") {
        let detail = stmt.get("result").map(std::string::ToString::to_string).unwrap_or_default();
        assert!(
            detail.contains("does not exist"),
            "read failed for a reason other than a missing table: {detail}"
        );
        return Vec::new();
    }
    stmt.get("result")
        .and_then(|r| r.as_array())
        .map(|a| a.iter().map(|r| r["body"].as_str().unwrap_or("<missing>").to_string()).collect())
        .unwrap_or_default()
}

/// Run `surreal export|import` against the unit the STRATEGY named.
fn surreal_io(verb: &str, ns: &str, db: &str, file: &str) {
    let out = Command::new(SURREAL)
        .args([verb, "--endpoint", EP, "-u", "root", "-p", "root", "--ns", ns, "--db", db, file])
        .output()
        .unwrap_or_else(|e| panic!("run surreal {verb}: {e}"));
    assert!(
        out.status.success(),
        "surreal {verb} --ns {ns} --db {db} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// **P-8.1, executed: restore one tenant, leave every other untouched.**
///
/// Run under `namespace-per-tenant`, the topology Chunk C adds — and the one
/// where "the tenant" and "the database" are least alike, since every tenant's
/// database is called `app`.
#[test]
fn one_tenant_restores_and_the_others_do_not_move() {
    let t = tenancy("namespace-per-tenant", Some("app"), None, &["wo040d_dr_a", "wo040d_dr_b"]);
    let a = t.resolve("wo040d_dr_a").expect("resolve a");
    let b = t.resolve("wo040d_dr_b").expect("resolve b");
    seed(&a, "a-original");
    seed(&b, "b-original");

    // THE PLAN COMES FROM THE STRATEGY, not from a guess about the shape
    let (ns, db, isolated) = frust_kernel::provision::export_target(&a);
    assert!(isolated, "namespace-per-tenant must promise a tenant-isolated restore");
    let db = db.expect("a database-scoped backup unit");
    assert_eq!((ns.as_str(), db.as_str()), ("wo040d_dr_a", "app"));

    let dir = std::env::temp_dir().join(format!("wo040d-dr-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    let dump = dir.join("tenant_a.surql");
    let dump = dump.to_str().expect("path");

    surreal_io("export", &ns, &db, dump);

    // Mutate BOTH tenants after the export, so "restored" and "untouched" are
    // claims with teeth rather than statements about an unchanged store.
    scoped_db(&a).sql_root("UPDATE ledger SET body = 'a-corrupted';").expect("corrupt a");
    scoped_db(&b)
        .sql_root("CREATE ledger SET owner_tenant = 'wo040d_dr_b', body = 'b-written-after-export';")
        .expect("write b");
    assert_eq!(bodies(&a), vec!["a-corrupted"]);
    assert_eq!(bodies(&b), vec!["b-original", "b-written-after-export"]);

    // **FINDING (WO-040 Chunk C): `surreal import` is ADDITIVE, not
    // restore-over.** Importing onto live data fails with "Database record
    // `ledger:…` already exists" — WO-027 and WO-039 both imported into a
    // *fresh* database and so never met this. So the real per-tenant restore
    // procedure has three steps, not two: **drop the target database, then
    // import into it.** Anyone who scripts export→import without the drop has
    // a restore that fails halfway through an incident.
    //
    // The drop is scoped to the unit the STRATEGY named, which is what keeps
    // it a per-tenant operation: under `namespace-per-tenant` it removes
    // `app` inside A's namespace only.
    scoped_db(&a)
        .sql_root_ns(&format!("REMOVE DATABASE {db}; DEFINE DATABASE {db};"))
        .expect("drop the restore target");
    assert!(bodies(&a).is_empty(), "the drop did not empty A");
    assert_eq!(
        bodies(&b),
        vec!["b-original", "b-written-after-export"],
        "dropping A's database disturbed B — the drop is not tenant-scoped"
    );

    surreal_io("import", &ns, &db, dump);
    let _ = std::fs::remove_dir_all(&dir);

    // A is back...
    assert_eq!(
        bodies(&a),
        vec!["a-original"],
        "tenant A did not restore to its exported state"
    );
    // ...and B never moved, including the write made AFTER the export
    assert_eq!(
        bodies(&b),
        vec!["b-original", "b-written-after-export"],
        "restoring A disturbed B — restore-one is still restore-all"
    );

    // and the provenance rule: every row A holds is A's own
    let owners = scoped_db(&a)
        .sql_root("SELECT VALUE owner_tenant FROM ledger;")
        .expect("owners");
    let owners: Vec<String> = owners
        .as_array()
        .map(|x| x.iter().map(|v| v.as_str().unwrap_or("?").to_string()).collect())
        .unwrap_or_default();
    assert_eq!(owners, vec!["wo040d_dr_a"], "A holds a row belonging to someone else");
}

/// **The control for the isolation claim.** `single` promises the opposite,
/// and the same strategy call must say so — otherwise the assertion above is
/// just reading a constant that happens to be `true` everywhere.
#[test]
fn the_shared_topology_refuses_to_promise_a_tenant_isolated_restore() {
    let t = tenancy("single", Some("wo040d_dr_shared"), None, &["wo040d_dr_x"]);
    let x = t.resolve("wo040d_dr_x").expect("resolve");
    let (_, _, isolated) = frust_kernel::provision::export_target(&x);
    assert!(
        !isolated,
        "`single` claimed restore-one; its database holds every tenant, and an operator \
         planning DR against that claim would take everyone down"
    );
}

/// **ADR-013 stays closed under a namespace topology.**
///
/// Chunk A found `keyguard.rs` forging its probe token with the tenant id as
/// the `db` claim — correct by coincidence under database-per-tenant, and
/// under a namespace topology it would name a database that does not exist,
/// so the store would refuse the forged token for the WRONG REASON and be
/// reported safe. This is that path, exercised where the two genuinely differ:
/// tenant `wo040d_kg_a`, database `app`.
#[test]
fn the_keyguard_probes_the_right_place_under_a_namespace_topology() {
    let t = tenancy("namespace-per-tenant", Some("app"), None, &["wo040d_kg_a"]);
    let target = t.resolve("wo040d_kg_a").expect("resolve");
    seed(&target, "kg");
    assert_ne!(
        target.tenant_id().as_str(),
        target.database().as_str(),
        "this test is only meaningful where tenant id and database differ"
    );

    let db = scoped_db(&target);
    // a key this store was NOT defined with must be refused...
    assert!(
        !keyguard::store_accepts_key(&db, "wo040d-not-the-key-000000000000")
            .expect("probe reached the store"),
        "the store accepted a key it was not defined with"
    );

    // ...and the refusal must be an AUTH refusal, not "no such database".
    // That is the whole finding: a probe aimed at a database that does not
    // exist is also refused, and would read as 'safe'.
    let forged = keyguard::forge_token(
        target.namespace().as_str(),
        target.database().as_str(),
        db.access(),
        "wo040d-not-the-key-000000000000",
    );
    let parts: Vec<&str> = forged.split('.').collect();
    assert_eq!(parts.len(), 3, "a JWT has three parts");
    let claims = String::from_utf8(
        base64_url_decode(parts[1]).expect("decode claims"),
    )
    .expect("utf8");
    assert!(
        claims.contains("\"db\":\"app\"") && claims.contains("\"ns\":\"wo040d_kg_a\""),
        "the probe token names the wrong location: {claims}"
    );
}

fn base64_url_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()
}
