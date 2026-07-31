---
tags: [frust, work-order, tier2, wasm]
status: completed — all gates passed 2026-07-24, Boa deviation ratified, → [[ADR-007 Tier-2 Script Architecture]]
created: 2026-07-24
---

# WO-001: Script-Engine Spike (Tier-2 Server Half)

> [!info] PM work order
> Gates **ADR-007 (Tier-2 Script Architecture)**. Builder: execute against the criteria below; write results to `D:\Dev\frust\04 Build Log\` — **verify the vault path before writing**.

## Accepted position (context, do not re-litigate)

- **Deleted subsystem:** no bespoke server-script engine. A user script = Tier-2-profiled invocation of [[ADR-006 Plugin Capability Surface]] — same hooks, same brokered verbs, narrower grants.
- **Mechanism:** scripts are data (stored in SurrealDB metadata, live-mutable — save script, next request runs it); the guest is one prebuilt, audited `script-engine.wasm` (JS engine on `wasm32-wasip2`) exposing the **same WIT hook exports as any plugin**.
- **Language: JS** (Frappe-user parity; one language across both Tier-2 halves).
- **Client half survives** per ADR-001: interaction verbs run in-browser via the six-verb bridge; no DOM access. Not part of this spike.

## Spike Specification

Engine candidate: StarlingMonkey or QuickJS-flavored component. Script under test: ~20-line `validate` (read fields, mutate own doc, typed reject path).

### Exit criteria

1. **Warm path < 1 ms** — engine instance alive, script bytecode cached; script-call round-trip through the WIT hook interface. *(The 166 µs from [[2026-07-24 WASM isolation spike]] priced a 93 KB toy — it does NOT transfer. Measure fresh.)*
2. **Cold path measured honestly, no gate** — engine instantiate + script compile, reported separately. This number *decides the instantiation policy inside ADR-007* (pool vs fresh-per-call vs bytecode cache): >20 ms cold makes caching load-bearing and it goes in the ADR, not after it.
3. **Containment through the engine layer** — infinite-loop script and unbounded-allocation script, killed by epoch/memory cap with host unaffected — same bar as the ADR-005 spike, now with a JS engine in between.
4. **Profile thesis proven structurally** — the existing wasm-spike host harness loads `script-engine.wasm` **unmodified** (same WIT world as plugin-demo). If it needs host changes, the "Tier 2 is a profile of ADR-006" claim is weaker than stated — report what changed.

### Deliverables

- [ ] Build log in `04 Build Log/` with all four results + numbers table
- [ ] Repro kept under `D:\Dev\rust\` (note the path in the log)
- [ ] Provenance check: confirm or retract the **620 ms Slow-4G** Topcoat interaction figure cited 2026-07-24 — it is load-bearing for the client/server split and currently uncorroborated in the vault

**On pass → I draft ADR-007** (the split + the profile table: which ADR-006 verbs each script class gets).
**On fail (1) → ** instantiation/caching architecture becomes the ADR's core; on fail (3) → the deletion thesis is dead, escalate before any further Tier-2 work.

## Related

[[Frust Hub]] · [[ADR-001 UI Extension Tiers]] · [[ADR-005 Plugin Isolation]] · [[ADR-006 Plugin Capability Surface]] · [[2026-07-24 WASM isolation spike]]
