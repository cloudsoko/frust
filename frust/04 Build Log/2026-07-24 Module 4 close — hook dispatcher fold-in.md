---
tags: [frust, build-log, kernel, work-order, hooks]
created: 2026-07-24
work-order: "[[WO-005 Metadata Kernel v0]]"
---

# Build Log — Module 4 Close: Hook Dispatcher Fold-In

## The fold-in proof (per module-1 orders)

`kernel/tests/permission_proof.rs::acceptance_write_through_hooks` — the WO-002 acceptance test — **passes with its assertions byte-for-byte unchanged.** The only edit is the `broker()` constructor: `ExternalHookRunner { endpoint: ":8787" }` → `WasmHooks::load(artifacts)`. That unchanged test IS the fold-in proof.

- `20000` Draft → `Needs Approval` @ `23000` (plugin flag + script tax): identical result, now computed in-process.
- Negative total → typed `HookRejected`: identical.
- The 2/1/3-row permission proof and byte-equality: untouched (hooks don't touch reads).

**`frust` boots reporting `hooks in-process (2-process deployment)` with zero hook-runner processes running.** Verified by killing every `hookrunner` process first. **Three processes are now two: `frust` + `surreal.exe`.**

## What changed

- **WIT v2** (`frust-kernel/wit/plugin.wit`, now the canonical copy): `validate` takes/returns `list<entry>` — the full dynamic doc (ADR-006 edge 3). The toy `{id,status,total}` shape is gone from the contract.
- **`kernel/src/hooks.rs`** — the in-process dispatcher: the ADR-005 spike host design (pooled instance per component, engine-global epoch ticker, 500 ms per-call deadline, 128 MiB memory cap, self-heal on trap) behind the broker's `HookDispatch` trait. Compiled plugin then Tier-2 script, chained on one validate.
- **`broker::db_write`** now sends the FULL document to the hooks and takes the mutated doc back verbatim — no field cherry-picking. The injected record `id` is a hook input, stripped before the write.
- **Both guests rebuilt** (`plugin-demo`, `script-engine`) against WIT v2. The script engine's Rust shell records each field's WIT-variant kind on the way in and re-imposes it on the way out, so a JS script cannot silently turn a decimal into a float.
- **`ExternalHookRunner` retained** as a `HookDispatch` impl that marshals the dynamic doc down to the legacy toy JSON and back — the toy shape now lives ONLY at that legacy boundary, nowhere in the broker.

## Design disclosures (for ADR-006)

- **WIT has no recursive types.** ADR-006's `value` tree carries nested list/object as a `compound-v(string)` JSON variant. Every *scalar* — crucially `decimal` — is first-class; only *nesting* is JSON-encoded at the wire. A wire-encoding note, not a semantic change: `hook_dispatch::decimal_stays_decimal_across_the_boundary` proves a decimal survives the round trip through the JS engine as a decimal, exactly (`"19.99"` in, `"19.99"` out).
- The dynamic envelope carries arbitrary fields the toy shape never could: `dynamic_doc_roundtrips_all_fields` sends `supplier_name`, `credit_limit` (decimal), `notes` through both hook classes and asserts they survive verbatim while `status` mutates.

## Tests added

- `hook_dispatch::dynamic_doc_roundtrips_all_fields`
- `hook_dispatch::decimal_stays_decimal_across_the_boundary`
- `hook_dispatch::negative_total_rejected`
- `hook_dispatch::dispatcher_self_heals_after_reject`

## Suite state

Full workspace green: **95 (frust-orm) + kernel tests** across 8 test binaries — unit (9), boot_discipline (4), conflict_canary (1), hook_dispatch (4), metadata_sync (4), permission_proof (6), surql_monopoly (1). The monopoly gate correctly did NOT flag `hooks.rs` (its `serde_json` compound handling contains no SurrealQL). Hook-heavy suites take ~30-45s (real wasmtime instantiation); everything else sub-2s.

## Carry-forwards

- Hook artifacts still load from `wasm-spike/artifacts/` (env-overridable via `FRUST_ARTIFACTS`); a kernel-owned build/embed of the components is a packaging concern for later, not a module-4 blocker.
- `enqueue` still inserts without claim/run — that's module 5.
- Hook `db-write` re-entrancy (a hook calling back into the broker) is wired through `HookChain` but not yet exercised end-to-end; arrives with the worker loop and the REST surface.

## Related
[[WO-005 Metadata Kernel v0]] · [[ADR-005 Plugin Isolation]] · [[ADR-006 Plugin Capability Surface]] · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-24 WASM isolation spike]] · [[2026-07-24 Module 3 close — sync engine port + rollback position]]
