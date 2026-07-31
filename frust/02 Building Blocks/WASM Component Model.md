---
tags: [frust, building-block, wasm, plugins]
status: adopted
created: 2026-07-24
---

# Building Block: WASM Component Model for Plugin Isolation

> [!success] Status: **adopted** — spike passed all three exit criteria 2026-07-24 ([[2026-07-24 WASM isolation spike]]), decision formalized in [[ADR-005 Plugin Isolation]]. The remaining hard work is not the runtime — it's **designing the WIT capability surface**, i.e. the plugin API itself.

## Why the Requirements Already Decided the Primary

- **REQ-2.1.2 kills embedded scripting (QuickJS/Boa/Rhai) as primary.** An in-process engine shares the host's memory space and scheduler — a runaway script blocks a tokio worker, and "can't corrupt core process memory" is exactly the guarantee it doesn't give. Frappe's server-script *UX* is worth stealing; its isolation model is on the pain-point list by name ([[Frappe Pain Points#5. Security|P-5.1]], sandboxed `eval` with a history of escapes).
- **Stack-collapse philosophy kills out-of-process as primary.** Process-per-app with IPC/gRPC recreates the supervisor zoo we're deleting (P-2.3, P-7.1), adds ~1 ms per hook call where `validate` needs microseconds, and turns "install an app" into an ops event. Strongest isolation, bought with the exact complexity Frust exists to remove.
- **WASM components are the same-shaped bet as [[SurrealDB]] and [[Topcoat]]** — one modern primitive collapsing a stack.

## What It Gives

| Capability | Requirement / pain point |
|---|---|
| Plugins are `.wasm` files loaded at runtime into one binary | [[SRS#2.1 App Packaging & Installation|REQ-2.1.1]] ✓ |
| wasmtime memory isolation + fuel/epoch CPU limits | REQ-2.1.2 ✓ — and a partial answer to P-8.2 tenant starvation |
| WIT interfaces = **typed** hook signatures (`before-insert(doc) -> result<doc>`) | REQ-2.2.1 — replaces hooks.py's global mutable magic (P-2.2) |
| Capability-based imports — a plugin *cannot* touch DB/network/files except via host functions we broker | P-2.1 — the permission chokepoint becomes **structural, not disciplinary** |

**Two bonuses:**
1. Brokered host functions mean every plugin DB call flows through the same permission-enforced gate as user requests — the `ignore_permissions=True` culture (P-3.3) becomes **unexpressible**, not discouraged.
2. The Tier-2 sandbox from [[ADR-001 UI Extension Tiers]] may converge on this same runtime (JS-in-WASM via StarlingMonkey-style engines) — **one sandbox story** for plugins *and* user scripts instead of two.

## Honest Costs (grill here)

- **The WIT surface IS the hard design problem.** Every DB/network/file capability must be hand-designed as a WIT interface — that's the plugin API, and it's authored, not free.
- **Async/host-call ergonomics in wasmtime are real work** (async host functions, store-per-request vs pooling, epoch interruption tuning).
- **Component-model tooling is young.** Debugging, versioned interface evolution, registry story — all early.
- **Author barrier:** compiling to `wasm32-wasip2` is fine for a curated marketplace, a real barrier for casual authors — which is exactly what Tier-2 scripts are for.
- **Fallback stays named:** out-of-process remains the escape hatch for plugin classes that genuinely need native code (ML runtimes, device drivers).

> [!warning] Reversed-bet flag
> First building block where the **primary technology is the immature one and the fallback is boring** — the reverse of SurrealDB/Topcoat, where the boring option didn't exist. So the spike's exit criteria carry more weight than usual.

## Spike Exit Criteria — ✅ all passed 2026-07-24

1. [x] Hook round-trip < 50 µs warm → **13.3 µs** (debug host build; release will be single-digit).
2. [x] Misbehaving plugin contained & killed → infinite loop stopped by epoch deadline (~200 ms, configurable); unbounded alloc trapped at 64 MiB cap in 22 ms; host unaffected, original instance still valid.
3. [x] Source → running plugin in one command → `cargo build --target wasm32-wasip2 --release`, 93 KB component.

Bonus: cold instantiate ~830 µs; fresh-instance-per-request 166 µs — lifecycle is a tuning knob. Findings: wasmtime API churn (WASI view moved in v37) → pin-and-deliberate-upgrade posture; avoid `u64::MAX` epoch deadlines (debug overflow, engine-global counters). Full log: [[2026-07-24 WASM isolation spike]].

## Related

[[Frust Hub]] · [[SRS]] · [[Frappe Pain Points]] · [[ADR-001 UI Extension Tiers]] · [[SurrealDB]] · [[Topcoat]]
