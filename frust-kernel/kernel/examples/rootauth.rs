//! Root-auth A/B: what the root JWT actually buys, measured.
//!
//! Runs against a **dedicated scratch store** (endpoint passed on the command
//! line) — never the live dev store's data directory, per standing policy.
//! Because it REMOVEs and recreates its benchmark database, it refuses to run
//! without `FRUST_SCRATCH=1` — an explicit confirmation that the target is a
//! throwaway store — and refuses the :8899 dev port outright. The port check
//! alone is not a safeguard: a live store on any other port would be wiped too.
//!
//!   FRUST_SCRATCH=1 cargo run --release --example rootauth -- http://127.0.0.1:8901
//!
//! Both arms run in ONE process against the SAME database, interleaved, with
//! ≥3 samples per arm. Interleaving is not decoration: an earlier
//! save-floor attempt compared arms measured at different times and produced
//! two contradictory numbers for one operation.

use std::time::{Duration, Instant};

use frust_kernel::db::{scoped_db, ConnConfig, Db};
use frust_kernel::tenancy::{Tenancy, TenancyConfig};

const SAMPLES: usize = 5;
const PER_SAMPLE: usize = 30;

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// One sample: `PER_SAMPLE` root queries, median latency.
fn sample(db: &Db, query: &str) -> Duration {
    let mut d = Vec::with_capacity(PER_SAMPLE);
    for _ in 0..PER_SAMPLE {
        let t = Instant::now();
        db.sql_root(query).expect("root query");
        d.push(t.elapsed());
    }
    median(d)
}

fn main() {
    let endpoint = std::env::args().nth(1).unwrap_or_else(|| "http://127.0.0.1:8901".into());
    if endpoint.contains("8899") {
        eprintln!("refusing to benchmark against the live dev store on :8899 — pass a scratch endpoint");
        std::process::exit(1);
    }
    // The setup below REMOVEs and recreates its benchmark database, so it must
    // never touch a store holding real data. The port check above is not enough
    // — a live store on any other port would be wiped just the same — so the
    // destructive run requires an explicit opt-in the operator cannot trip by
    // accident.
    if std::env::var("FRUST_SCRATCH").ok().as_deref() != Some("1") {
        eprintln!(
            "refusing to REMOVE/recreate the benchmark database at {endpoint} without explicit \
             confirmation — set FRUST_SCRATCH=1 to confirm this endpoint is a throwaway scratch store"
        );
        std::process::exit(1);
    }
    let name = "wo044_bench";
    let conn = ConnConfig { endpoint: endpoint.clone(), ..ConnConfig::default() };
    let cfg = TenancyConfig {
        strategy: "single",
        namespace: Some("frust"),
        database: Some(name),
        environment: None,
        tenants: &[name],
    };
    let tenancy = Tenancy::from_config(conn, cfg).expect("tenancy");
    let target = tenancy.sole().expect("sole");

    // ── a fresh database in the scratch store ──
    let setup = scoped_db(&target);
    setup
        .sql_root_ns(&format!("REMOVE DATABASE IF EXISTS {name}; DEFINE DATABASE {name};"))
        .expect("fresh database");
    setup
        .sql_root(
            "DEFINE TABLE bench SCHEMALESS PERMISSIONS NONE; \
             CREATE bench:a SET v = 1; CREATE bench:b SET v = 2; CREATE bench:c SET v = 3;",
        )
        .expect("seed");

    println!("root-auth A/B saturation probe — endpoint {endpoint}, {SAMPLES} samples × {PER_SAMPLE} queries\n");

    let query = "SELECT * FROM bench;";
    let mut jwt_samples = Vec::new();
    let mut basic_samples = Vec::new();

    // Two handles onto the same database, one per credential. Warm both first
    // so neither arm pays a first-connection or first-signin cost inside a
    // measured sample.
    let jwt_db = scoped_db(&target);
    let mut basic_db = scoped_db(&target);
    basic_db.force_basic_for_test();
    for _ in 0..5 {
        jwt_db.sql_root(query).expect("warm jwt");
        basic_db.sql_root(query).expect("warm basic");
    }
    assert!(jwt_db.has_cached_root_token(), "the jwt arm must hold a token");
    assert!(!basic_db.has_cached_root_token(), "the basic arm must not");

    for i in 0..SAMPLES {
        // interleaved, so drift and background load hit both arms alike
        let j = sample(&jwt_db, query);
        let b = sample(&basic_db, query);
        println!("  sample {}: jwt {j:>10.3?}  basic {b:>10.3?}", i + 1);
        jwt_samples.push(j);
        basic_samples.push(b);
    }

    let j = median(jwt_samples.clone());
    let b = median(basic_samples.clone());
    let spread = |v: &[Duration]| {
        let mut s = v.to_vec();
        s.sort();
        (s[0], s[s.len() - 1])
    };
    let (jlo, jhi) = spread(&jwt_samples);
    let (blo, bhi) = spread(&basic_samples);

    println!("\n  JWT   median {j:?}   (samples {jlo:?} … {jhi:?})");
    println!("  BASIC median {b:?}   (samples {blo:?} … {bhi:?})");
    #[allow(clippy::cast_precision_loss)]
    let ratio = b.as_secs_f64() / j.as_secs_f64();
    println!("\n  → {ratio:.1}× faster, {:?} saved per root query", b.saturating_sub(j));
    // The token is per ENDPOINT, not per handle, so the arm that happens to
    // mint it is whichever `Db` queries first — here the setup handle. Printing
    // all three makes the sharing visible: three handles, ~300 root queries,
    // ONE argon2 verification for the whole process.
    let total_signins = setup.root_signins() + jwt_db.root_signins() + basic_db.root_signins();
    println!(
        "  → signins across all handles: {total_signins} (setup {}, jwt {}, basic {}) for {} root queries",
        setup.root_signins(),
        jwt_db.root_signins(),
        basic_db.root_signins(),
        SAMPLES * PER_SAMPLE * 2 + 10 + 4
    );
    assert_eq!(total_signins, 1, "the whole run must cost exactly one signin");

    // Convergence check: if the samples within an arm disagree wildly, the
    // medians are not comparable and the number should not be reported.
    let jspread = jhi.as_secs_f64() / jlo.as_secs_f64().max(f64::MIN_POSITIVE);
    let bspread = bhi.as_secs_f64() / blo.as_secs_f64().max(f64::MIN_POSITIVE);
    println!("  → sample spread: jwt {jspread:.2}×, basic {bspread:.2}×");
    if jspread > 3.0 || bspread > 3.0 {
        println!("  !! SAMPLES DID NOT CONVERGE — do not report this delta");
        std::process::exit(2);
    }
}
