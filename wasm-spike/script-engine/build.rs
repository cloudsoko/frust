// WO-030: make the kernel's `decimal.rs` includable into this guest without a
// shared crate. `include!` forbids inner doc-comments (`//!`), and the kernel
// file opens with a module-header block of them — so copy the canonical file
// into OUT_DIR each build, demoting only its leading `//!` lines to `//`. The
// arithmetic is byte-for-byte the kernel's; `rerun-if-changed` means any edit
// to the source re-copies, so the three hosts cannot drift.
use std::{env, fs, path::Path};

fn main() {
    let src = "../../frust-kernel/kernel/src/decimal.rs";
    println!("cargo:rerun-if-changed={src}");
    let text = fs::read_to_string(src).expect("read kernel decimal.rs");
    let demoted: String = text
        .lines()
        .map(|l| if l.trim_start().starts_with("//!") { l.replacen("//!", "// ", 1) } else { l.to_string() })
        .collect::<Vec<_>>()
        .join("\n");
    let out = Path::new(&env::var("OUT_DIR").unwrap()).join("decimal.rs");
    fs::write(out, demoted).expect("write demoted decimal.rs");
}
