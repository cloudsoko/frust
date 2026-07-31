//! **WO-040 Chunk B: the SHIPPED BINARY refuses a bad tenancy config.**
//!
//! `tenancy.rs`'s unit tests prove `Tenancy::from_config` rejects incomplete
//! and contradictory settings. That is a proof about a function. This is the
//! proof that `frust` actually *calls* it — the standing lesson that a tested
//! seam the product never takes is not a tested product.
//!
//! Every case here spawns the real binary and asserts a **non-zero exit**
//! plus the reason in its log. All of them are decided before the kernel
//! touches the database, so they are fast and need no fixture.
//!
//! The last case is the control: the same binary, a valid config, **exit 0**.
//! Without it, six processes exiting 1 would prove only that the binary exits
//! 1 — which it would also do if it were simply broken.
//!
//! The control needs the dev surreal on :8899; the refusals do not.

use std::process::Command;

fn run(env: &[(&str, &str)], args: &[&str]) -> (Option<i32>, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_frust"));
    // never inherit the operator's own tenancy settings into a test that is
    // about tenancy settings
    for k in ["FRUST_TENANCY", "FRUST_TENANT", "FRUST_TENANTS", "FRUST_DATABASE", "FRUST_NS"] {
        cmd.env_remove(k);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.args(args).output().expect("spawn frust");
    let log = String::from_utf8_lossy(&out.stdout).to_string()
        + &String::from_utf8_lossy(&out.stderr);
    (out.status.code(), log)
}

fn refuses(case: &str, env: &[(&str, &str)], expect: &str) {
    let (code, log) = run(env, &[]);
    assert_eq!(code, Some(1), "{case}: expected a refusal, got exit {code:?}\n{log}");
    assert!(
        log.contains("tenancy_refused"),
        "{case}: exited 1 but did not report a tenancy refusal — it may have failed for an \
         unrelated reason:\n{log}"
    );
    assert!(log.contains(expect), "{case}: refusal did not say {expect:?}:\n{log}");
}

/// **WO-040 Chunk B2 inverted this test.** Chunk B refused a roster larger
/// than one, because per-request routing did not exist and serving only the
/// first name would have been the silent misbehaviour. B2 built the routing,
/// so the same config must now **boot** — and boot *every* tenant on it, not
/// just the first.
#[test]
fn a_multi_tenant_roster_now_boots_every_tenant_on_it() {
    const A: &str = "wo040c_boot_a";
    const B: &str = "wo040c_boot_b";
    for db in [A, B] {
        let target = frust_kernel::tenancy::single_tenant(db).expect("tenancy");
        frust_kernel::db::scoped_db(&target)
            .sql_root_ns(&format!("DEFINE DATABASE IF NOT EXISTS {db};"))
            .expect("provision");
    }

    let (code, log) =
        run(&[("FRUST_TENANTS", &format!("{A},{B}"))], &["--accept-meta-migrations"]);
    assert_eq!(code, Some(0), "a two-tenant roster did not boot:\n{log}");
    for db in [A, B] {
        assert!(
            log.contains(&format!("\"tenant\":\"{db}\"")),
            "tenant {db} was on the roster but never booted — a subset was served:\n{log}"
        );
    }
}

#[test]
fn an_empty_roster_entry_refuses_rather_than_being_trimmed_away() {
    // a stray comma is a typo; quietly serving fewer tenants than were named
    // is exactly what this seam exists to prevent
    refuses("empty entry", &[("FRUST_TENANTS", "acme,")], "empty entry");
}

#[test]
fn an_incomplete_single_topology_refuses() {
    refuses(
        "single without a database",
        &[("FRUST_TENANCY", "single"), ("FRUST_TENANT", "acme")],
        "incomplete",
    );
}

#[test]
fn a_contradictory_database_per_tenant_config_refuses() {
    refuses(
        "database-per-tenant with a shared database",
        &[("FRUST_TENANCY", "database-per-tenant"), ("FRUST_DATABASE", "everyone")],
        "contradicts itself",
    );
}

/// **WO-040 Chunk C inverted this one too.** Chunk B refused the namespace
/// topologies as "not built into this binary"; Chunk C built them, so a
/// correct config must now boot — and the boot line must say which topology
/// and *where*, because that is what an operator checks.
#[test]
fn a_namespace_topology_now_boots_and_reports_where_it_put_the_tenant() {
    const T: &str = "wo040d_boot_ns";
    let tenancy = frust_kernel::tenancy::Tenancy::from_config(
        frust_kernel::db::ConnConfig::default(),
        frust_kernel::tenancy::TenancyConfig {
            strategy: "namespace-per-tenant",
            namespace: None,
            database: Some("app"),
            environment: None,
            tenants: &[T],
        },
    )
    .expect("tenancy");
    frust_kernel::provision::provision_all(&tenancy).expect("provision");

    let (code, log) = run(
        &[("FRUST_TENANCY", "namespace-per-tenant"), ("FRUST_DATABASE", "app"), ("FRUST_TENANT", T)],
        &["--accept-meta-migrations"],
    );
    assert_eq!(code, Some(0), "a namespace topology did not boot:\n{log}");
    assert!(log.contains("\"tenancy\":\"namespace-per-tenant\""), "topology not reported:\n{log}");
    // the namespace IS the tenant, and the database is the configured one
    assert!(log.contains(&format!("\"ns\":\"{T}\"")), "wrong namespace in the boot line:\n{log}");
    assert!(log.contains("\"db\":\"app\""), "wrong database in the boot line:\n{log}");
}

/// Each namespace topology still refuses when the one setting it *does* need
/// is missing — the fail-closed half of the same change.
#[test]
fn a_namespace_topology_refuses_without_the_setting_it_needs() {
    refuses(
        "namespace-per-tenant without a database",
        &[("FRUST_TENANCY", "namespace-per-tenant")],
        "no database named",
    );
    refuses(
        "namespace-per-tenant-env without an environment",
        &[("FRUST_TENANCY", "namespace-per-tenant-env")],
        "does not know whether it is production",
    );
    refuses(
        "an environment that is not one",
        &[("FRUST_TENANCY", "namespace-per-tenant-env"), ("FRUST_ENVIRONMENT", "prod-2")],
        "not a deployment environment",
    );
}

#[test]
fn an_unknown_topology_refuses() {
    refuses("unknown", &[("FRUST_TENANCY", "elastic")], "unknown tenancy topology");
}

/// **The control.** Same binary, valid config, boots and exits clean — so the
/// refusals above are the config being rejected, not the binary being broken.
///
/// The database is provisioned first because the kernel refuses to boot into
/// one that does not exist (ADR-008's fail-closed boot, `E_BOOT_DB`). That
/// refusal is correct and is *not* what this file is testing — provisioning
/// is a separate act from configuring, and the kernel is right not to
/// conflate them.
#[test]
fn the_same_binary_boots_clean_on_a_valid_config() {
    const DB: &str = "wo040b_boot_ok";
    let target = frust_kernel::tenancy::single_tenant(DB).expect("tenancy");
    frust_kernel::db::scoped_db(&target)
        .sql_root_ns(&format!("DEFINE DATABASE IF NOT EXISTS {DB};"))
        .expect("provision");

    let (code, log) = run(&[("FRUST_TENANT", DB)], &["--accept-meta-migrations"]);
    assert_eq!(code, Some(0), "a valid config did not boot:\n{log}");
    assert!(log.contains("\"evt\":\"booted\""), "no boot line:\n{log}");
    assert!(
        log.contains("\"tenancy\":\"database-per-tenant\""),
        "boot did not report which topology it chose — an operator cannot see what they got:\n{log}"
    );
    assert!(!log.contains("tenancy_refused"), "clean boot still logged a refusal:\n{log}");
}
