//! **The tenancy bypass guard** (WO-040 Chunk A, ADR-003 invariant 4).
//!
//! `surql_monopoly` proved the posture works: an invariant that lives only in
//! a reviewer's head decays, and one that fails the build does not. This is
//! the same guard for the other monopoly — *nothing outside `tenancy.rs`
//! decides which namespace or database a query lands in.*
//!
//! Chunk A migrates today's database-per-tenant behaviour onto the seam. What
//! stops it drifting back off is this file: the day someone adds a
//! `Db::for_database(name)` "just for a script", or reaches for a
//! `surreal-db` header in a new module, or hardcodes the namespace again, the
//! build fails with the line number.
//!
//! **The scanner is proved to fail.** `the_guard_catches_a_planted_bypass`
//! runs the same `scan` over synthetic sources containing each bypass and
//! asserts every rule fires. A guard that has never been seen to reject
//! anything is not yet a guard — it is a green light with no bulb.

use std::path::Path;

/// `db.rs` is the transport: it is the one module that may put a namespace or
/// database on the wire — and it can only get them off a `ResolvedTenant`.
/// `realtime.rs` is the WS transport, same reasoning for the `use` RPC.
/// `tenancy.rs` is the boundary itself. `main.rs` is startup: it is where a
/// process is *told* which topology and tenant it serves.
///
/// Every entry is a deliberate ruling, not a convenience.
struct Rule {
    /// what a violation looks like in source
    needle: &'static str,
    /// the only files allowed to contain it
    allowed: &'static [&'static str],
    why: &'static str,
}

const RULES: &[Rule] = &[
    // ── 1. no direct ns/db selection on the wire ──
    Rule {
        needle: "surreal-ns",
        allowed: &["db.rs"],
        why: "namespace selection belongs to db.rs, off a ResolvedTenant",
    },
    Rule {
        needle: "surreal-db",
        allowed: &["db.rs"],
        why: "database selection belongs to db.rs, off a ResolvedTenant",
    },
    Rule {
        needle: "\"ns\":",
        allowed: &["db.rs", "keyguard.rs"],
        why: "an ns claim/param is placement; it comes from the strategy",
    },
    Rule {
        needle: "\"db\":",
        allowed: &["db.rs", "keyguard.rs"],
        why: "a db claim/param is placement; it comes from the strategy",
    },
    Rule {
        needle: "call(\"use\"",
        allowed: &["realtime.rs"],
        why: "use ns/db on a shared client is the classic cross-tenant bug",
    },
    // ── 2. no hardcoded ns/db constants ──
    Rule {
        needle: "\"frust\"",
        allowed: &["tenancy.rs"],
        why: "the namespace is config, not a literal",
    },
    Rule {
        needle: "\"skeleton\"",
        allowed: &["tenancy.rs"],
        why: "the default tenant is config, not a literal",
    },
    // ── 3. only tenancy.rs mints a target ──
    //
    // A `ResolvedTenant { .. }` literal outside the seam is caught by the
    // COMPILER (private fields), which is stronger than a text scan — so
    // there is no needle for it. What a text scan does add is
    // `targets_cannot_be_assembled` below: it fails if someone makes those
    // fields `pub`, which is the change that would quietly re-enable the
    // literal everywhere at once.
    Rule {
        needle: "StorageLocation {",
        allowed: &["sync.rs"],
        why: "the migrator's location is projected from the strategy (sync.rs::scope_of)",
    },
    // ── 3b. WO-040 Chunk B: nothing downstream knows the topology ──
    //
    // The tell that a strategy abstraction is genuine rather than the old
    // single implementation wearing a trait: no code outside the seam can
    // name a strategy. The moment `if strategy == "single"` appears
    // anywhere, the abstraction has leaked and every caller starts having
    // to know which topology it is running under.
    Rule {
        needle: "SingleTenant",
        allowed: &["tenancy.rs"],
        why: "downstream must never branch on which topology it runs under",
    },
    Rule {
        needle: "DatabasePerTenant",
        allowed: &["tenancy.rs"],
        why: "downstream must never branch on which topology it runs under",
    },
    Rule {
        needle: "\"database-per-tenant\"",
        allowed: &["tenancy.rs"],
        why: "a strategy name in a comparison is the abstraction leaking",
    },
    // ── 4. provisioning is startup, never a request ──
    Rule {
        needle: "Tenancy::from_config",
        allowed: &["tenancy.rs", "main.rs"],
        why: "registering a tenant from request data would defeat the registry",
    },
    Rule {
        needle: "Tenancy::from_env",
        allowed: &["tenancy.rs", "main.rs"],
        why: "registering a tenant from request data would defeat the registry",
    },
    Rule {
        needle: "single_tenant(",
        allowed: &["tenancy.rs", "main.rs"],
        why: "registering a tenant from request data would defeat the registry",
    },
];

/// Flag a source file's violations. Shared by the real scan and the
/// planted-bypass proof, so the two cannot disagree about the rules.
fn scan(name: &str, content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue; // prose about a bypass is not a bypass
        }
        for rule in RULES {
            if trimmed.contains(rule.needle) && !rule.allowed.contains(&name) {
                out.push(format!("{name}:{}: `{}` — {}", i + 1, rule.needle, rule.why));
            }
        }
    }
    out
}

#[test]
fn no_module_outside_the_seam_selects_a_namespace_or_database() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    visit(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "tenancy bypass — route this through `tenancy::Tenancy` + `db::scoped_db`:\n{}",
        violations.join("\n")
    );
}

/// The other half of the monopoly: **there is no bare `db()` fallback.**
///
/// `scoped_db(&ResolvedTenant)` is the only constructor, and an overload that
/// took a database *name* would reopen the whole bypass while looking like a
/// convenience — so its absence is asserted, not assumed.
#[test]
fn db_has_exactly_one_constructor_and_it_takes_a_resolved_tenant() {
    let db_rs = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("db.rs"),
    )
    .expect("read db.rs");

    let constructors: Vec<&str> = db_rs
        .lines()
        .map(str::trim_start)
        .filter(|l| !l.starts_with("//"))
        .filter(|l| l.contains("-> Db"))
        .collect();

    assert_eq!(
        constructors.len(),
        1,
        "db.rs must expose exactly one way to build a Db; found:\n{}",
        constructors.join("\n")
    );
    assert!(
        constructors[0].contains("scoped_db(target: &ResolvedTenant)"),
        "the one constructor must take a ResolvedTenant, got: {}",
        constructors[0]
    );
}

/// **The guard, shown to fail.**
#[test]
fn the_guard_catches_a_planted_bypass() {
    // one planted source per rule, in a module that is NOT on its allowlist
    let planted: &[(&str, &str)] = &[
        ("broker.rs", r#"    .header("surreal-db", "tenant_b")"#),
        ("broker.rs", r#"    .header("surreal-ns", "frust")"#),
        ("worker.rs", r#"    let body = json!({ "ns": ns });"#),
        ("worker.rs", r#"    let body = json!({ "db": db });"#),
        ("rest.rs", r#"    conn.call("use", json!([ns, db]))?;"#),
        ("boot.rs", r#"    let ns = "frust";"#),
        ("rest.rs", r#"    let db = "skeleton";"#),
        ("app.rs", r#"    let loc = StorageLocation { namespace: n, database: d };"#),
        ("broker.rs", r#"    let s = SingleTenant { namespace: ns, database: db };"#),
        ("rest.rs", r#"    if self.strategy.is::<DatabasePerTenant>() { .. }"#),
        ("worker.rs", r#"    if strategy == "database-per-tenant" { fan_out() }"#),
        ("rest.rs", r#"    let t = Tenancy::from_config(conn, s, ns, &[slug])?;"#),
        ("rest.rs", r#"    let t = Tenancy::from_env(conn, &[slug])?;"#),
        ("rest.rs", r#"    let target = single_tenant(&payload.tenant)?;"#),
    ];

    for (file, line) in planted {
        let hits = scan(file, line);
        assert!(
            !hits.is_empty(),
            "the guard did NOT catch a planted bypass in {file}: {line}"
        );
    }

    // every rule is exercised above — a rule nobody plants is a rule nobody
    // has seen work
    assert_eq!(planted.len(), RULES.len(), "each rule needs a planted bypass");

    // and it must not cry wolf: the same lines inside a comment, and the same
    // needles in their allowed homes, are clean
    assert!(scan("broker.rs", r#"    // .header("surreal-db", "x")"#).is_empty());
    assert!(scan("db.rs", r#"    .header("surreal-db", d)"#).is_empty());
}

/// The compiler already refuses a `ResolvedTenant { .. }` literal outside the
/// seam — because its fields are private. **This asserts they stay private.**
/// Publishing them is a one-word change that would silently restore every
/// bypass the seam closes, and it is the kind of change that reads as
/// harmless in review.
#[test]
fn targets_cannot_be_assembled() {
    let src = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("tenancy.rs"),
    )
    .expect("read tenancy.rs");

    let body = src
        .split_once("pub struct ResolvedTenant {")
        .expect("ResolvedTenant struct")
        .1
        .split_once("\n}")
        .expect("struct end")
        .0;

    let public: Vec<&str> =
        body.lines().map(str::trim_start).filter(|l| l.starts_with("pub ")).collect();
    assert!(
        public.is_empty(),
        "ResolvedTenant's fields must stay private — only `tenancy` may mint a target:\n{}",
        public.join("\n")
    );
}

fn visit(dir: &Path, violations: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            visit(&path, violations);
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("rs") {
            continue;
        }
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let content = std::fs::read_to_string(&path).expect("read file");
        violations.extend(scan(&name, &content));
    }
}
