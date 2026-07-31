---
tags: [frust, adr, tier2, scripts, wasm]
status: accepted
decided: 2026-07-24
---

# ADR-007: Tier-2 Scripts — A Profile of the Plugin Surface, Not a Subsystem

**Context:** [[ADR-001 UI Extension Tiers]] promised a Tier-2 sandbox "when built." [[WO-001 Script Engine Spike]] passed all four gates ([[2026-07-24 Script engine spike (WO-001)]]). This ADR realizes Tier 2 and **deletes a subsystem**: Frust never builds a server-script engine.

## Decision

**One script language (JS), one hook-point vocabulary (the metadata schema's, per ADR-001), two hosts, zero bespoke sandboxes:**

- **Server half:** a user script is a Tier-2-profiled invocation of [[ADR-006 Plugin Capability Surface]]. The guest is one prebuilt, audited `script-engine.wasm` exposing the same WIT hook exports as any plugin (proven: unmodified harness, one disclosed CLI-arg change). Scripts are **data** — stored in SurrealDB metadata, live-mutable: save script → next request runs it, zero deploys (proven: same 4 MB component, different script text, no recompile).
  > [!note] Live-mutability semantics precised (WO-048, 2026-07-31): "save script" means **through a kernel door** (`POST /doctype`, app install/update — all of which bump the meta generation). An **out-of-band** edit (direct DB write, bypassing the kernel) is not seen until the next generation bump — pinned as a stated contract by `an_out_of_band_script_edit_is_not_seen_until_the_generation_bumps`. This is the one-door principle applied to script delivery, not a narrowing of the ratified property: direct DB writes were never a supported door.
- **Client half:** the same JS dialect, same hook-point names, executing in-browser through ADR-001's six-verb bridge; no DOM access, no data verbs. Justified by the **corroborated** 620 ms/interaction Slow-4G measurement — dependent-field logic cannot round-trip.
- Frappe's RestrictedPython server scripts (P-5.1, sandbox escapes by name) get **no analog** — the pain point dies by deletion.

## Profile Table — which verbs each execution class gets

| Verb | Client script | Doc-hook script | Scheduled script | Compiled plugin (ADR-006) |
|---|---|---|---|---|
| Six-verb form bridge | ✅ | — | — | — |
| Mutate own doc (typed return) | via bridge `set-value` | ✅ | — | ✅ |
| `db-read` | ❌ (bridge `call-server`) | ✅ | ✅ | ✅ |
| `db-aggregate` | ❌ | ✅ | ✅ | ✅ |
| `db-named-query` | ❌ | ✅ | ✅ | ✅ |
| `db-write` | ❌ | ❌ — own doc via return only | ✅ (hooks fire; ADR-006 cycle rules apply) | ✅ |
| `enqueue` | ❌ | ✅ | ✅ | ✅ |
| `log` | ✅ | ✅ | ✅ | ✅ |

*API-endpoint scripts deferred — not a v1 class. Widening any cell is an ADR amendment, not a config option.*

## Operational Decisions (from spike findings)

1. **Instantiation policy: pooled per `(tenant, script-set)`** — 55–67 µs/call warm. Fresh instance is ~7 ms (761 µs instantiate + 6.3 ms first call), so script edits are effectively free and bytecode caching is *recommended, not load-bearing*. Component compile (8.1 s) happens once per engine deploy, cached.
2. **Per-hook epoch deadline ALONGSIDE the memory cap, never instead of it** — the allocation bomb ground for **10.4 s** before hitting the memory cap; the epoch deadline is what bounds wall-clock.
3. **Error hygiene:** the engine strips JS-engine internals (Boa stack-trace suffixes) from WIT error strings before they reach users.
4. **Engine: Boa** (pure-Rust, same one-command toolchain as plugins) — *current, not married*. Chosen under a no-wasi-sdk constraint; slowest candidate, so all gate numbers are conservative. The WIT world is the contract; StarlingMonkey/QuickJS remain drop-in candidates if warm latency ever matters.

> [!warning] Scripting-docs item (WO-009 finding, 2026-07-25)
> **Decimals reach Tier-2 scripts as strings** (the ADR-006 wire encoding preserving exactness). `doc.total + tax` string-concatenates → NaN → silently nulls the field. Required discipline, to be front-and-center in user scripting docs: `Number()` (or a provided decimal helper) on the way in, same-type out. The demo script models it. Consider a lint/runtime warning when a decimal field receives a NaN/type-changed value — that turns the silent failure loud, which is this project's house style.

> [!note] Money DISPLAY formatting is permitted — padding is not arithmetic (ruling 2026-07-30, built WO-046)
> SurrealDB strips trailing zeros at write, so `37.50` is **stored** as `37.5`.
> That is faithful storage and this ADR's compare-never-compute rule is
> untouched by it — but it is not what a customer should read on an invoice
> (WO-043's approval emails and WO-045's printed invoice both surfaced it).
>
> **Ruled: padding a stored decimal string to the field's currency scale for
> DISPLAY is a presentation concern and is permitted.** It never touches the
> stored value, never parses to a float, and never recomputes a total — so it
> is not arithmetic, and nothing in this ADR is weakened.
>
> **Display-rounding is never needed, and is forbidden.** Money is stored *at*
> scale, so a value carrying MORE places than the scale is a **defect to
> surface, not to silently round** — `1.005` in a two-place field means
> something upstream is wrong, and a display layer that quietly printed `1.01`
> would hide it at the one moment a human could still catch it.
>
> Implemented as `pad_money` in the Desk (WO-046), first used by the document
> view; unit-pinned including the over-scale pass-through, and asserted
> in-browser to leave the stored value unchanged. **Scale is 2 everywhere
> today** — DocType metadata carries no `precision`, so a per-field scale (and a
> currency symbol) remain **print-metadata vocabulary for the ADR-014
> follow-on**, named rather than hardcoded per doctype.

## Evidence

Warm call 55.7 µs (17× under the 1 ms gate; ~4× native-plugin's 13.3 µs). JS infinite loop epoch-killed; allocation bomb memory-capped; host unaffected. JS `throw` → typed reject. Full numbers: [[2026-07-24 Script engine spike (WO-001)]].

**Related:** [[Frust Hub]] · [[ADR-001 UI Extension Tiers]] · [[ADR-005 Plugin Isolation]] · [[ADR-006 Plugin Capability Surface]] · [[WO-001 Script Engine Spike]]
