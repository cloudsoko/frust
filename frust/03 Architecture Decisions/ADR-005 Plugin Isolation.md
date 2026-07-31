---
tags: [frust, adr, wasm, plugins]
status: accepted
decided: 2026-07-24
---

# ADR-005: Plugin Isolation — WASM Components Primary

**Context:** REQ-2.1.1 (runtime app loading), REQ-2.1.2 (isolated execution), REQ-2.2.1 (lifecycle hooks). Option space and elimination reasoning in [[WASM Component Model]]; empirical gate passed in [[2026-07-24 WASM isolation spike]].

**Decision:**
- **Primary: WASM component model on wasmtime.** Plugins are `.wasm` components loaded at runtime; WIT-typed hook signatures; capability-based imports — DB/network/files reachable *only* through brokered host functions, making the permission chokepoint structural (P-2.1, P-3.3).
- **Fallback (named, not vestigial): out-of-process** for plugin classes genuinely needing native code (ML runtimes, device drivers).
- **Rejected as primary: embedded scripting engines** — in-process = no memory/scheduler isolation; fails REQ-2.1.2 and repeats P-5.1. (Scripting UX returns as ADR-001 Tier 2, likely JS-in-WASM on this same runtime.)

**Evidence:** 13.3 µs warm hook round-trip (debug build, target was 50 µs); hostile plugins (infinite loop, unbounded alloc) contained and killed with the host unaffected; source→component in one command at 93 KB.

**Operational posture:** pin wasmtime, deliberate upgrades only (API churn observed v36→v37). Epoch deadlines finite. Instance lifecycle (pooled vs fresh-per-request at 166 µs) is a per-hook-class tuning decision, deferred.

**What this ADR does NOT decide:** the WIT capability surface (`db-read`, `db-write`, `enqueue`, …) — that is the plugin API proper, the real remaining design work. Grill first, then ADR-006.

**Related:** [[Frust Hub]] · [[WASM Component Model]] · [[ADR-001 UI Extension Tiers]] · [[2026-07-24 WASM isolation spike]]
