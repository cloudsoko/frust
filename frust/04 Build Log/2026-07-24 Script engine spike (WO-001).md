---
tags: [frust, build-log, tier2, wasm, work-order]
created: 2026-07-24
work-order: "[[WO-001 Script Engine Spike]]"
---

# Build Log — Script-Engine Spike (WO-001)

**Repro:** `D:\Dev\rust\wasm-spike\script-engine\` (guest: Boa JS engine + the ~20-line `validate.js` under `scripts/`), measured with the ADR-005 harness (`host/src/main.rs`) and `host/src/bin/coldbench.rs`. wasmtime 37, debug-built host (numbers are a ceiling).

**Disclosed deviation from spec:** engine is **Boa** (pure-Rust JS), not StarlingMonkey/QuickJS — no wasi-sdk C toolchain on the build machine, and Boa builds with the identical one-command `cargo build --target wasm32-wasip2` as any plugin. Boa is the *slower* engine of the three, so gates passed here pass with margin for the named candidates; it is not license-encumbered (MIT/Apache-2). Engine choice remains swappable behind the WIT world — ADR-007 need not marry Boa.

## Results vs exit criteria

| # | Criterion | Result |
|---|---|---|
| 1 | Warm script call < 1 ms (GATE) | ✅ **55.7 µs** (main harness, 100k calls) / 67.3 µs (coldbench, 20k) — ~17× under the gate. Includes doc marshaling both ways + JS execution. Native-plugin baseline was 13.3 µs → JS costs ~4×. |
| 2 | Cold path measured (no gate) | Component compile **8.1 s** (once per deploy; cacheable via precompile). Store+instantiate **761 µs**; first call (JS ctx + parse + exec) **6.3 ms** → full per-fresh-instance cold ≈ **7 ms**. Under the WO's 20 ms caching-becomes-load-bearing threshold. |
| 3 | Containment through the JS engine | ✅ JS `while(true){}` killed by epoch deadline (~200 ms, configurable); JS allocation bomb trapped at the 64 MiB store cap; host survived both, original instance kept serving, 110k brokered `log` calls processed. |
| 4 | Unmodified harness loads it | ✅ Structurally proven. **One disclosed harness change:** the component *path* became a CLI argument (was hardcoded to plugin-demo). Loading, linking, bindings, limits, and the test suite are byte-identical. The engine exports the same `frust:plugin` world; `validate` runs the JS script, JS `throw` surfaces as the WIT `result` error. |

**Scripts-are-data proven:** same component, host-supplied script text (via WASI env, standing in for SurrealDB-stored metadata) changed behavior — a strict per-tenant script rejected the doc with its own message. No recompile, no new world. Script delivery mechanism (env at instance creation vs a dedicated set-script capability) is an ADR-007 detail; env-at-creation implies a script edit = new instance ≈ 7 ms, negligible at script-editing frequency.

## Deliverable 3 — 620 ms provenance: **CONFIRMED**

Fresh reproduction today, same methodology as 2026-07-23: Chrome DevTools **Slow 4G** preset (~165 ms RTT), click-to-DOM-swap on the frust-proto shard grid (`/grid`, "next" button, table-content change as the swap signal), 5 samples: **617 / 619 / 621 / 625 / 628 ms** (original run: 615–624 ms). The figure is real and stable. Related context from the original run: Fast 4G ≈ 215–221 ms, loopback 14–18 ms, swap ≈ 3–4× RTT (multiple round trips per shard fetch — separate upstream question).

## Notes for ADR-007

- **Instantiation policy (per criterion 2's mandate):** pooled long-lived instance per (tenant, script-set) → 55–67 µs calls; fresh-per-call (~7 ms) affordable only for low-frequency hooks; component compile amortized once per deploy with wasmtime's cache. Caching is *recommended*, not load-bearing — the 20 ms threshold was not crossed.
- **Both containment knobs compose and both are needed:** the allocation bomb ground for 10.4 s before hitting the memory cap (JS-side allocation is slow) — a production hook deadline (epoch ticks) caps wall-time regardless of which resource the script burns.
- **Error surface needs shaping:** a JS `throw` currently carries Boa's trace suffix (`at <main> …`) into the WIT error string — the engine should strip/structure this before users see it.
- The engine component is 4.0 MB (vs 93 KB native toy) — irrelevant at rest, relevant only to compile-once time.

## WO checklist

- [x] Build log with all four results + numbers
- [x] Repro kept under `D:\Dev\rust\wasm-spike\` 
- [x] 620 ms figure: confirmed by reproduction (retraction not needed)

**All gates passed → ADR-007 drafting is unblocked.**

## Related

[[WO-001 Script Engine Spike]] · [[ADR-005 Plugin Isolation]] · [[ADR-006 Plugin Capability Surface]] · [[2026-07-24 WASM isolation spike]] · [[2026-07-23 Topcoat prototype]]
