//! `frust serve` — the metadata kernel, resident. Modules 1-6:
//! broker + boot discipline + ported sync engine + in-process hooks +
//! worker loop + REST surface. Two-process deployment: `frust` + surreal.exe.
//!
//! **WO-040 Chunk B2: one process, N tenants.** Every tenant on the roster is
//! booted (ADR-008 meta discipline is per database, so it runs per tenant),
//! gets its own `Broker`, `MetadataSync` and worker — all sharing **one**
//! wasmtime engine — and the REST surface routes each request to its own
//! tenant's context by splitting the bearer token.

use std::sync::Arc;

use frust_kernel::boot::{boot, BootOptions};
use frust_kernel::broker::{Broker, HookDispatch};
use frust_kernel::db::{scoped_db, ConnConfig, Db};
use frust_kernel::hooks::WasmHooks;
use frust_kernel::rest::Rest;
use frust_kernel::router::{TenantContext, TenantRouter};
use frust_kernel::sync::MetadataSync;
use frust_kernel::telemetry;
use frust_kernel::tenancy::{ResolvedTenant, Tenancy};
use frust_kernel::worker::{AppUserResolver, ResidentWorker, Worker};

fn refuse(evt: &str, detail: String) -> ! {
    telemetry::emit(telemetry::Level::Error, evt, &[("error", serde_json::json!(detail))]);
    std::process::exit(1);
}

fn report_recovery(report: String) {
    telemetry::emit(
        telemetry::Level::Info,
        "recovery_complete",
        &[("report", serde_json::json!(report))],
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let recovery = match frust_kernel::recovery::parse(&args[1..]) {
        Ok(command) => command,
        Err(e) => refuse("recovery_refused", e.to_string()),
    };
    let serve = args.iter().any(|a| a == "serve");
    let accept = args.iter().any(|a| a == "--accept-meta-migrations");

    // Integrity verification is deliberately offline: it needs neither root
    // credentials nor a topology, and remains usable during a DB outage.
    if matches!(
        &recovery,
        Some(frust_kernel::recovery::RecoveryCommand::Verify { .. })
    ) {
        match frust_kernel::recovery::run(recovery.expect("matched verify"), None) {
            Ok(report) => {
                report_recovery(report);
                return;
            }
            Err(e) => refuse("recovery_refused", e.to_string()),
        }
    }

    // The topology is chosen HERE, once, from validated config — and a config
    // this binary cannot honour refuses the boot rather than quietly serving a
    // different one (ADR-008's fail-closed posture, applied to tenancy).
    //
    // `FRUST_TENANTS` (comma-separated) is the roster; `FRUST_TENANT` is the
    // one-tenant spelling. Blank entries are refused rather than trimmed away:
    // a stray comma is a typo, and a config that quietly serves fewer tenants
    // than it names is the silent misbehaviour this whole seam exists against.
    let roster = std::env::var("FRUST_TENANTS")
        .or_else(|_| std::env::var("FRUST_TENANT"))
        .unwrap_or_else(|_| {
            recovery
                .as_ref()
                .and_then(frust_kernel::recovery::RecoveryCommand::tenant)
                .unwrap_or(frust_kernel::tenancy::DEFAULT_TENANT)
                .to_string()
        });
    let slugs: Vec<&str> = roster.split(',').map(str::trim).collect();
    if slugs.iter().any(|s| s.is_empty()) {
        refuse("tenancy_refused", format!("FRUST_TENANTS has an empty entry: {roster:?}"));
    }

    let tenancy = match Tenancy::from_env(ConnConfig::default(), &slugs) {
        Ok(t) => t,
        Err(e) => refuse("tenancy_refused", e.to_string()),
    };
    let mut targets: Vec<ResolvedTenant> = match tenancy.resolve_all() {
        Ok(t) => t,
        Err(e) => refuse("tenancy_refused", e.to_string()),
    };
    // deterministic order, so logs and the sole-tenant case read the same way
    // on every boot
    targets.sort_by(|a, b| a.tenant_id().as_str().cmp(b.tenant_id().as_str()));

    // Recovery uses the same validated registry and strategy as serving, but
    // never boots hooks/workers or opens the REST listener. Restore has its
    // own post-import keyguard and cannot fall through to ordinary boot.
    if let Some(command) = recovery {
        match frust_kernel::recovery::run(command, Some(&tenancy)) {
            Ok(report) => {
                report_recovery(report);
                return;
            }
            Err(e) => refuse("recovery_refused", e.to_string()),
        }
    }

    // ── ONE engine for the whole process (WO-040 criterion 2) ──
    //
    // WO-039's finding: owned hooks would have meant N wasmtime engines and N
    // epoch tickers, multiplying the 60 MB idle footprint by tenant count. The
    // script pool inside `WasmHooks` is already `(tenant, doctype)`-keyed, so
    // one engine serves every tenant.
    let artifacts = std::env::var("FRUST_ARTIFACTS")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts").to_string());
    // WO-022 FINDING: the resident kernel must attach the per-DocType script
    // source, or `frust serve` runs only the engine's built-in default and an
    // app's server scripts never fire live. The script source is a `Db`, and
    // the pool keys by tenant, so it is scoped to the first target and the
    // pool key keeps tenants apart.
    let hooks: Arc<dyn HookDispatch> = match WasmHooks::load(&artifacts) {
        Ok(h) => Arc::new(h.with_script_source(scoped_db(&targets[0]))),
        Err(e) => refuse("hooks_unavailable", e.to_string()),
    };

    // ── boot every tenant: ADR-008's meta discipline is per DATABASE ──
    let opts = BootOptions { accept_meta_migrations: accept, ..Default::default() };
    let mut brokers: Vec<Arc<Broker>> = Vec::with_capacity(targets.len());
    for target in &targets {
        let db = scoped_db(target);
        let sync = MetadataSync { base: target.clone() };
        let report = match boot(&db, &opts, &sync) {
            Ok(r) => r,
            Err(e) => refuse("boot_refused", format!("{}: {e}", target.tenant_id())),
        };
        telemetry::emit(telemetry::Level::Info, "booted", &[
            ("tenancy", serde_json::json!(tenancy.strategy().name())),
            ("tenant", serde_json::json!(target.tenant_id().as_str())),
            ("ns", serde_json::json!(target.namespace().as_str())),
            ("db", serde_json::json!(target.database().as_str())),
            ("meta_version", serde_json::json!(report.meta_version)),
            ("meta_applied", serde_json::json!(report.applied_meta)),
            ("doctypes", serde_json::json!(report.doctypes)),
        ]);
        telemetry::gauge(
            "frust_meta_version",
            &[("tenant", target.tenant_id().as_str())],
            report.meta_version as f64,
        );
        brokers.push(Arc::new(Broker::with_shared_hooks(scoped_db(target), Arc::clone(&hooks))));
    }
    telemetry::gauge("frust_build_info", &[("version", env!("CARGO_PKG_VERSION"))], 1.0);
    telemetry::gauge("frust_tenants_served", &[], targets.len() as f64);

    if !serve {
        telemetry::emit(telemetry::Level::Info, "one_shot_boot_done", &[]);
        return;
    }

    // ── the router: one context per tenant ──
    //
    // Built from the SAME registry the strategy resolved, so a token prefix
    // that finds a context here has been through `TenancyStrategy::resolve`.
    let mut made = brokers.iter().cloned();
    let router = match TenantRouter::build(&tenancy, |target| {
        Ok(TenantContext {
            broker: made.next().expect("one broker per resolved tenant"),
            sync: Some(Arc::new(MetadataSync { base: target.clone() })),
        })
    }) {
        Ok(r) => Arc::new(r),
        Err(e) => refuse("tenancy_refused", e.to_string()),
    };

    // ── per-tenant background work ──
    //
    // Owned `Db`s in a vec that outlives `serve`, so the workers can borrow
    // them. One resident worker and one set of rollup workers per tenant: a
    // job queue and a changefeed cursor live in each tenant's own database.
    let worker_dbs: Vec<Db> = targets.iter().map(scoped_db).collect();
    let resolvers: Vec<AppUserResolver> =
        worker_dbs.iter().map(|db| AppUserResolver { db }).collect();

    let mut residents = Vec::with_capacity(targets.len());
    let mut rollups = Vec::new();
    for (i, target) in targets.iter().enumerate() {
        let worker_db = &worker_dbs[i];
        residents.push(ResidentWorker {
            worker: Worker::new(worker_db, &brokers[i], &format!("resident-{i}")),
            resolver: &resolvers[i],
        });

        // ADR-010 Tier-2: rollup workers from metadata declarations. An
        // unknown handler name is a boot refusal — a declared rollup silently
        // not running would be staleness with no bound.
        for dt in frust_kernel::sync::load_doctypes(worker_db).unwrap_or_default() {
            for agg in &dt.aggregates {
                if agg.kind != "worker" {
                    continue;
                }
                let handler: Box<dyn frust_kernel::aggregates::Contrib> = match agg.handler.as_deref() {
                    Some("group_revenue") => {
                        Box::new(frust_kernel::aggregates::GroupRevenue::new(&dt.name, &agg.rollup, "grp"))
                    }
                    Some("item_sales") => {
                        Box::new(frust_kernel::aggregates::ItemSales::new(&dt.name, &agg.rollup))
                    }
                    other => refuse(
                        "unknown_rollup_handler",
                        format!("{other:?} on {} in tenant {}", dt.name, target.tenant_id()),
                    ),
                };
                match frust_kernel::aggregates::RollupWorker::new(worker_db, handler) {
                    Ok(w) => rollups.push(w),
                    Err(e) => refuse("rollup_worker_init_failed", e.to_string()),
                }
            }
        }
    }

    // ── WO-043: the mail worker, on its OWN thread ──
    //
    // Its own thread, not the maintenance closure below: a blocking SMTP send
    // against a dead relay waits out its timeout, and doing that on the
    // maintenance thread would stall rollup drains and the realtime reaper — a
    // dead SMTP server would slow the whole background tier instead of just
    // email. One thread serves every tenant in turn, because the cost of mail is
    // the network wait, not the loop.
    //
    // The transport is resolved ONCE here and an unusable configuration REFUSES
    // THE BOOT (ADR-008's fail-closed posture): a kernel that starts up and only
    // reveals at the first invoice that `FRUST_MAIL` was misspelled has told the
    // operator nothing when it could have told them everything.
    let mailer = match frust_kernel::mail::Mailer::from_env() {
        Ok(m) => Arc::new(m),
        Err(e) => refuse("mail_config_refused", e.to_string()),
    };
    telemetry::emit(telemetry::Level::Info, "mail_transport", &[
        ("mode", serde_json::json!(mailer.mode())),
        ("target", serde_json::json!(mailer.target())),
    ]);
    telemetry::gauge("frust_mail_transport", &[("mode", mailer.mode())], 1.0);
    {
        let mail_dbs: Vec<Db> = targets.iter().map(scoped_db).collect();
        let mailer = Arc::clone(&mailer);
        std::thread::spawn(move || loop {
            for (i, db) in mail_dbs.iter().enumerate() {
                let w = frust_kernel::worker::MailWorker {
                    db,
                    mailer: &mailer,
                    worker_id: format!("mail-{i}"),
                };
                if let Err(e) = w.drain() {
                    telemetry::emit(telemetry::Level::Error, "mail_drain_failed", &[
                        ("tenant", serde_json::json!(db.tenant_id())),
                        ("error", serde_json::json!(e.to_string())),
                    ]);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        });
    }

    let addr = std::env::var("FRUST_ADDR").unwrap_or_else(|_| "127.0.0.1:8790".into());
    let realtime = Arc::new(frust_kernel::realtime::Realtime::new(targets[0].endpoint()));
    let rest = Rest {
        router: Arc::clone(&router),
        addr,
        realtime: Some(Arc::clone(&realtime)),
    };
    // `move`: the maintenance closure OWNS the resident workers, the rollup
    // workers and the realtime handle, and runs on exactly one thread
    // (WO-025). Owning rather than borrowing is what makes it `Send` without
    // demanding `Sync` from state that is single-threaded by design.
    if let Err(e) = rest.serve(move || {
        for r in &residents {
            let _ = r.tick();
        }
        for r in &rollups {
            let _ = r.drain();
        }
        realtime.reap();
    }) {
        refuse("serve_failed", e.to_string());
    }
}
