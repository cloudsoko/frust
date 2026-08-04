//! What fuel metering costs the hook path, in µs.
//! The perf gate measures whole milliseconds and reads 0 for hooks â€” too
//! coarse to answer "what did metering cost". Run explicitly:
//!
//!   cargo test --release --test fuel_overhead -- --ignored --nocapture

use frust_kernel::broker::HookDispatch;
use frust_kernel::contract::Value;
use frust_kernel::hooks::WasmHooks;

fn artifacts() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

#[test]
#[ignore = "measurement run"]
fn fuel_metering_overhead_on_the_hook_path() {
    let hooks = WasmHooks::load(artifacts()).expect("load components");
    let doc = vec![
        ("id".to_string(), Value::Text("thing:1".into())),
        ("status".to_string(), Value::Text("Draft".into())),
        ("total".to_string(), Value::Float(100.0)),
    ];
    for _ in 0..200 {
        hooks.validate("thing", &doc).unwrap();
    }
    let mut samples = Vec::with_capacity(2000);
    for _ in 0..2000 {
        let t = std::time::Instant::now();
        hooks.validate("thing", &doc).unwrap();
        samples.push(t.elapsed().as_nanos() as u64);
    }
    samples.sort_unstable();
    let p50 = samples[samples.len() / 2] as f64 / 1000.0;
    let p95 = samples[samples.len() * 95 / 100] as f64 / 1000.0;
    println!(
        "hook chain (plugin + script) WITH fuel metering: p50 {p50:.1} us, p95 {p95:.1} us\n\
         (baseline without metering: ~55.7 us warm; script-hook floor: 1000 us)"
    );
    assert!(p50 < 1000.0, "hook chain stays inside the 1 ms script-hook floor: {p50:.1} us");
}

