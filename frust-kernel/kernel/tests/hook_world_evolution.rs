//! The capability surface's additive evolution policy, spent for the first time.
//!
//! The policy was written on day two — *the capability surface grows
//! additively; old components keep working* — and has never once been tested
//! against the real toolchain. Growing the hook vocabulary is the first change
//! that actually asks it a question.
//!
//! The load-bearing proof is NOT that a new component works. It is that a
//! component built against the OLD world still loads and runs, receiving only
//! the events it exports. The fixtures in `wasm-spike/artifacts-old-world/`
//! are exactly that: the shipped `plugin_demo.wasm` and `script_engine.wasm`
//! as they stood before the hook vocabulary grew, snapshotted by hash.

use frust_kernel::contract::Value;
use frust_kernel::hooks::WasmHooks;

/// The components as they existed before the vocabulary grew. Not rebuilt
/// here — that is the entire point.
fn old_world() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts-old-world")
}

/// The current build.
fn current() -> &'static str {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../../wasm-spike/artifacts")
}

/// **THE COMPATIBILITY PROOF.** An old component still loads under the grown
/// world.
///
/// If this fails, the evolution policy has met reality and lost, and that is a
/// design-decision conversation — not something to work around here.
#[test]
fn a_component_built_against_the_old_world_still_loads() {
    let loaded = WasmHooks::load(old_world());
    match &loaded {
        Ok(_) => println!("old-world components loaded"),
        Err(e) => println!("old-world components REFUSED: {e:?}"),
    }
    assert!(
        loaded.is_ok(),
        "the surface must grow ADDITIVELY — an existing component must keep \
         loading. It did not: {:?}",
        loaded.err()
    );
}

/// And it still *runs* — loading is not the same as working. A component that
/// links but whose one wired export now dispatches to nothing would satisfy the
/// test above while being broken.
#[test]
fn an_old_component_still_runs_the_hook_it_does_export() {
    let hooks = WasmHooks::load(old_world()).expect("old components load");
    let doc = vec![
        ("id".to_string(), Value::Text("inv:probe".into())),
        ("status".to_string(), Value::Text("Draft".into())),
        ("total".to_string(), Value::Decimal("20000".into())),
    ];
    let out = frust_kernel::broker::HookDispatch::validate(&hooks, "inv", &doc);
    println!("old component validate -> {out:?}");
    assert!(out.is_ok(), "an old component's validate must still run: {out:?}");
}

/// The current artifacts load too — the control. Without it, "old components
/// load" could be reporting that the loader accepts anything.
#[test]
fn the_current_artifacts_load_as_well() {
    assert!(WasmHooks::load(current()).is_ok(), "the shipped artifacts must load");
}
