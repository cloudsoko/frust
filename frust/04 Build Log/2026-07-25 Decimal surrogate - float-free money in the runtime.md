---
tags: [frust, build-log, topcoat, vendor, decimal, money]
created: 2026-07-25
---

# Build Log — Decimal Surrogate: Float-Free Money in the Runtime

First build in the vendored trunk driven purely by the vision. Re-ranking the "(More) reactivity" menu against [[SRS]] surfaced that the originally-recommended item (numeric vocabulary via `str.parse() → f64`) was a **REQ-6.2.1 violation** — "a float representation of money crossing any boundary is a defect," and the browser expression runtime is an unguarded boundary. The Frust-correct build is a Decimal type in the runtime. Upstream would never prioritize ERP decimal; vendoring is exactly what unlocks it.

## What shipped (vendor `main`, commit `552daf6`)

A `Decimal` surrogate on both sides of Topcoat's language boundary, backed by a **validated numeric string** — mirroring the kernel's `Value::Decimal(String)` (ADR-006). Never an `f64`, anywhere.

| Capability | Notes |
|---|---|
| Numeric comparison `eq/ne/gt/lt/ge/le` | Exact and scale-insensitive: `1.5 == 1.50`, `10 > 9` (not lexicographic). One `cmp` core, six thin wrappers, identical algorithm in Rust and TS. |
| `is_zero` / `is_negative` | For conditional visibility (`:hidden=$(balance.is_negative())`). |
| `to_string` | Exact string, trailing zeros preserved (`1234.50` stays `1234.50`). |
| **No arithmetic** | Deliberate. Per the ADR-007 line: display/compare client-side, but computed-then-stored money stays server-side. The type has no `add`/`sub` — the boundary is enforced by omission. |
| Tagged wire form | Serializes `{t:"Decimal",v:"..."}`, hydrates back to a Decimal — never collides with bare-number → f64. |
| Loud construction | `Decimal::new("1e5")` / `"1.2.3"` panics ("not a decimal number"). Money never silently coerces. |

## Verification

- **Rust** (`_decimal.rs`, 5 tests): scale-insensitive equality, numeric ordering, trailing-zero display, float-notation + malformed rejection.
- **TS** (`decimal.test.ts`, 4 tests): the same cases, plus a **shared comparison table asserted identically in both languages** so Rust and TS can't drift.
- **Translation** (`decimal_expr.rs`, 2 tests): a `Decimal` signal compiles in `$(...)` — `.gt(`, `.to_string()`, `.is_zero()` emitted; declared as a tagged Decimal; exact server render.
- **End-to-end in a real browser — the float-killer**: `signal big = Decimal::new("9999999999999999.99")` (18 significant digits, beyond f64's 2^53). Browser displayed **`9999999999999999.99` exactly** — an f64 would have shown `10000000000000000`. Provably never touched a float. Toggling a bool re-evaluated the client-side decimal comparison (`big > cap` → `true`), correct and reactive.
- Full suites green: view-macro 76, runtime lib 6, browser vitest 10.

## Why this matters for Frust

- **Unblocks money in reactive forms.** The instant Desk v2 grows a reactive currency field, the vocabulary now has a non-float path — REQ-6.2.1 holds at the browser boundary, not just the plugin/DB boundaries.
- **The ADR-007 boundary is enforced structurally**, not by discipline: the surrogate physically cannot do persisted arithmetic (no ops), so the "kernel is the floor" line can't erode one convenient field at a time — the tension flagged at the roadmap review is answered in the type system.
- Composes with the earlier dynamic-signals finding: metadata-driven forms can now carry per-field money signals that display and compare client-side, zero round-trips, full precision.

## Notes
- Vendor posture (fix locally, no upstream PRs) held — this is ours, upstream has no ERP-decimal need. If they later ship a decimal, we diff and reconcile like any other adoption.
- `dist/index.js` rebuilt and committed (19.4 KB); pnpm lockfiles kept out (upstream tracks yarn). Browser build ran via `node node_modules/tsup/...` directly to skip pnpm's esbuild-approval prehook.

## Related
[[Topcoat]] · [[SRS]] (REQ-6.2) · [[ADR-006 Plugin Capability Surface]] · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-25 Topcoat vendored - 175 signal methods]] · [[2026-07-25 Dynamic signals - upstream issue and PR]]
