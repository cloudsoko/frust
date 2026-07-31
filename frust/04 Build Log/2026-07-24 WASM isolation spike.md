---
tags: [frust, build-log, wasm, plugins]
created: 2026-07-24
---

# Build Log: WASM Isolation Spike

**Goal:** pass/fail the three exit criteria in [[WASM Component Model#Spike Exit Criteria]].
**Where:** `D:\Dev\rust\wasm-spike\` — WIT contract (`wit/`), guest plugin (`plugin-demo/`), wasmtime-37 host (`host/`).

## Results — all three criteria pass

| Criterion | Target | Measured |
|---|---|---|
| (a) Warm hook round-trip | < 50 µs | **13.3 µs** on a *debug* host build; release will be single-digit |
| (b) Hostile plugin contained | host survives | ✅ infinite loop killed by epoch deadline (~200 ms, configurable); unbounded allocator trapped at 64 MiB cap in 22 ms; host kept serving, original instance stayed valid |
| (c) Source → running plugin | one command | ✅ `cargo build --target wasm32-wasip2 --release` → **93 KB** component |

**Bonus datapoints:** cold instantiate ~830 µs; fresh-instance-per-request (full isolation) only **166 µs** — instance lifecycle is a tuning knob, not a constraint.

**Thesis exercised end-to-end:** typed WIT `validate` signature; doc mutation (`Draft` → `Needs Approval`); typed rejection; a brokered `log` capability as the plugin's *only* window to the world.

## Honest findings

- **wasmtime API churn is real** — WASI view API moved again in v37; pin-and-deliberate-upgrade posture, same as SurrealDB/Topcoat.
- Debug-only epoch-deadline overflow when passing `u64::MAX` (engine-global counters) — use finite deadlines.

## Follow-ups

- [ ] Design the real WIT capability surface (`db-read`, `db-write`, `enqueue`, …) — **this is the actual plugin API and the hard design problem**; grill before writing
- [ ] Re-measure (a) on a release host build
- [ ] Decide instance lifecycle policy (pooled warm vs fresh-per-request at 166 µs)

## Related

[[Frust Hub]] · [[WASM Component Model]] · [[ADR-005 Plugin Isolation]] · [[SRS]]
