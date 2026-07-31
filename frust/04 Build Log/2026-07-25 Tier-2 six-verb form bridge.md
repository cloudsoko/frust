---
tags: [frust, build-log, topcoat, vendor, tier2, forms]
created: 2026-07-25
---

# Build Log — Tier-2 Six-Verb Form Bridge (REQ-2.2.3)

The niche-defining mechanism, built and proven in a real browser. This is the client half of [[ADR-007 Tier-2 Script Architecture]] and the runtime realization of [[ADR-001 UI Extension Tiers]]'s Tier-2 promise: a metadata-driven form where each field carries its own client state, and a small verb set manipulates it. The verbs are what a sandboxed user script will attach to Tier-1 hook points; here they are driven by handlers to prove the mechanism.

Vendor `main`, commit `6ef3a59`; runnable reference at `examples/frust-form-bridge`.

## The six verbs — proven in-browser, verb by verb

| Verb | Maps to | Round-trip | Proof |
|---|---|---|---|
| get-value | read a field's value signal | **no** | drives the rules below |
| set-value | write a field's value signal | **no** | "Copy quantity to notes" → Notes became `25` |
| toggle-visibility | write a field's visible signal | **no** | selecting *wholesale* revealed the Discount % row |
| toggle-read-only | write a field's readonly signal | **no** | "Lock notes" → notes input `disabled: true`, others untouched |
| validate | write a field's error signal | **no** | quantity `0` → "Quantity must be positive" inline |
| call-server | a `#[procedure]` | **YES (once)** | "Check stock" → `widget: 46 units in stock` |

**The decisive measurement — the network log.** After exercising all six verbs, the browser had made **exactly one** non-static request: the `call-server` procedure POST. Every other verb ran with **zero** round-trips. That is the "dependent-field logic cannot round-trip at 620 ms Slow-4G" problem, solved: five of six verbs never touch the network.

## The architectural correction (flag for ratification)

> [!important] ADR-001 constraint note is now FALSE — proposed amendment
> ADR-001 (and its prototype note) says: *"Topcoat signals are compile-time items — no per-metadata-field signals. The Tier-2 six-verb bridge must compile to generic operations (shard re-render of a form section on driver-field change, ~15 ms measured), not per-field signal wiring."*
>
> The dynamic-signals finding ([[2026-07-25 Dynamic signals - upstream issue and PR]]) disproved the premise, and this build **depends on** per-field signals: every metadata field gets its own value/visible/readonly/error signal, created in a runtime loop. The consequence is strictly better than the ADR assumed — the bridge is **zero-round-trip for five of six verbs**, not a ~15 ms shard re-render per driver change. Proposed: amend ADR-001 to record per-field signals as the bridge's mechanism, and downgrade the shard-re-render path to a fallback for cases a signal can't express.

## How the bridge is built

The bridge is the **per-field signal harness** over Topcoat primitives (no new framework internals): for each metadata field, four signals — `value`, `visible`, `readonly`, `error` — created from runtime metadata in a loop (the per-field-signal pattern). Bind attributes read them (`:hidden`, `:disabled`, error text); the verbs write them. Hook points are handler expressions: the `@change`/`@input` on a driver field is its `on_change`. `call-server` is a `#[procedure]` — the sole networked verb, matching ADR-007's "client scripts get no data verbs, only call-server."

**ADR-007 boundary held structurally:** money stays a `Decimal` and is never computed client-side. The bridge does display/visibility/validation in the browser; any value that gets *stored* is the server's job. The Decimal surrogate has no arithmetic, so this can't be violated by accident.

## Findings (vocabulary gaps the bridge surfaced)

1. **No `||` / `&&` in the expression language.** `is_empty() || == "0"` is a parse error ("unsupported operator"); the grammar handles arithmetic + comparison only, not logical combinators. Worked around with `else if`. **Candidate vendor addition** (small, same shape as the Decimal work): `BoolSurrogate.and`/`.or` + grammar mapping for `&&`/`||`. Real ergonomic gap for validation rules.
2. **Async closures can't capture external signals cleanly** — an owned `Signal` used in both a `SignalDeclaration` and an `async` handler hit E0382/E0597. Fix: declare such signals with the view `signal x = …` statement (local binding) rather than an external `Signal`. So `call-server` result signals use the statement form; per-field (looped) signals stay external and are only touched by sync handlers. Worth a runtime note; not blocking.

## What's next on this rung
- **The sandbox** (running untrusted user-script *text* against these verbs) is the deferred-risk piece ADR-001 explicitly defers. The honest path is ADR-007's "two hosts, one dialect": load `script-engine.wasm` in the browser with the six-verb bridge as its host API. This build is its target surface — the bridge exists; the sandbox drives it later.
- **Fold into the Desk**: WO-009's `frust-proto` stays server-rendered; Desk v2 adopts this harness for reactive forms, wiring the field signals to kernel record data and `call-server` to the kernel's `db-read`.

## Notes
- Committed as a runnable example (like `frust-proto`); CI builds it, guarding compile regressions. No runtime-source change, so no `dist` rebuild.
- Vendor posture held: local build, no upstream PR.

## Related
[[ADR-001 UI Extension Tiers]] · [[ADR-007 Tier-2 Script Architecture]] · [[SRS]] (REQ-2.2.3) · [[2026-07-25 Dynamic signals - upstream issue and PR]] · [[2026-07-25 Decimal surrogate - float-free money in the runtime]] · [[Topcoat]]
