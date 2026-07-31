//! PM note 2 on module 1: "the only place in the kernel that assembles
//! query text" is an invariant, not a current fact — this test enforces it
//! in CI. Modules allowed to contain SurrealQL keywords are listed here;
//! anything else touching query text fails the build.

use std::path::Path;

// meta.rs: static hardcoded DDL only — no dynamic values ever enter it.
// sync.rs: the kernel's derive-equivalent (DocType -> DDL templates); every
//   interpolation is identifier-validated before embedding.
// Every addition here is a deliberate ruling, not a convenience.
// Every entry is a deliberate ruling, not a convenience: surql.rs (the
// compiler), db.rs (transport), broker.rs (verbs via surql helpers), boot.rs
// (meta under the lock), meta.rs (static DDL only), sync.rs (DocType->DDL),
// worker.rs (queue claim/finish, all values escaped via surql.rs),
// aggregates.rs (WO-007 ruling: Tier-2 feed replay + delta upserts — table
// idents validated once in RollupWorker::new, keys via surql::escape_str,
// record ids via surql::render_value),
// rest.rs (WO-009 ruling: the session store + meta/audit/lag routes — every
// doctype/rollup is ident()-checked, tokens are charset-validated AND
// escaped, user/role/jwt go through surql::escape_str; data routes still
// speak only the broker).
const ALLOWED: [&str; 10] = [
    "surql.rs", "db.rs", "broker.rs", "boot.rs", "meta.rs", "sync.rs", "worker.rs",
    "aggregates.rs", "rest.rs",
    // provision.rs (WO-040 chunk C ruling): tenant provisioning DDL. Every
    // value it interpolates is a typed NamespaceName/DatabaseName whose
    // constructor is private to `tenancy` and accepts nothing but a plain
    // identifier — the parameters CANNOT hold a client string, so this is a
    // stronger guarantee than "the caller remembers to escape".
    "provision.rs",
];
const KEYWORDS: [&str; 8] =
    ["SELECT ", "CREATE ", "UPDATE ", "UPSERT ", "DELETE ", "DEFINE ", "REMOVE ", "SHOW CHANGES"];

#[test]
fn only_allowed_modules_touch_query_text() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    visit(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "SurrealQL text outside the allowed modules (route it through surql.rs / broker):\n{}",
        violations.join("\n")
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
        if ALLOWED.contains(&name.as_str()) {
            continue;
        }
        let content = std::fs::read_to_string(&path).expect("read file");
        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            for kw in KEYWORDS {
                if trimmed.contains(kw) {
                    violations.push(format!("{name}:{}: {}", i + 1, trimmed));
                }
            }
        }
    }
}
