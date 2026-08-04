//! "No bare `println!` survives" — enforced in CI like the surql
//! monopoly. Kernel output is structured JSON-lines through `telemetry` or
//! it does not ship. (Tests and the orm crate are out of scope; the gate
//! guards the kernel's own log stream.)

use std::path::Path;

#[test]
fn kernel_src_has_no_bare_prints() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    visit(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "bare print in kernel src (route it through telemetry::emit):\n{}",
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
        let content = std::fs::read_to_string(&path).expect("read file");
        for (i, line) in content.lines().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            if trimmed.contains("println!(") || trimmed.contains("eprintln!(") {
                violations.push(format!("{name}:{}: {trimmed}", i + 1));
            }
        }
    }
}
