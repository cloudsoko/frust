---
tags: [frust, build-log, money, aggregates, work-order]
created: 2026-07-26
work-order: "[[WO-016 Decimal Rollup Accumulation]]"
---

# Build Log — WO-016: Decimal Rollup Accumulation

Money is decimal end-to-end through the aggregates layer, proven with the sums `f64` provably gets wrong. **Criterion 2's risky unknown came back clean — and the probe found something bigger on the way.**

## Criterion 2 first (the empirical rule, applied before designing)

Probed on v3.2.0 before writing any code:

| Probe | Result |
|---|---|
| `decimal + decimal` inside an EVENT body | **stays decimal** — `is_decimal: true`, `is_float: false`, value exactly `0.3` |
| float control (does the test bite?) | `0.1f + 0.2f` → `0.30000000000000004f`; `(0.1f + 0.2f) = 0.3f` → **False** |
| decimal control | `(0.1dec + 0.2dec) = 0.3dec` → **True** |
| our actual DDL shape `(NONE ?? 0) + 0.1dec` | **decimal** — the int zero promotes, it does not degrade |
| `0.1f + 0.1dec` (mixed) | **decimal** — SurrealDB promotes *toward* decimal |

**No silent decimal→float coercion. No pillar finding; ADR-002's taxonomy question does not fire.** Tier-1 EVENT counters were already decimal-safe *provided the field is decimal-typed* — which turned out to be the actual problem.

## The bigger finding the probe exposed

`Currency` fields were declared **`TYPE float`** in `doctype_ddl` — every money column in every DocType, not just rollups (`total: 115.0, is_float: true`, DDL `TYPE none | float`). The rollup was one symptom of a system-wide root cause: money became a float the moment it landed, so Tier-1's decimal-safe arithmetic was operating on floats anyway.

One mapping changed it: **`TYPE decimal` / `TYPE option<decimal>`, `ASSERT $value >= 0dec`.** Narrow change, wide effect — flagged here because the blast radius is every DocType, and because it means criterion 1 could not have been satisfied by touching the aggregates layer alone.

## Exit criteria

| # | Criterion | Result |
|---|---|---|
| 1 | Decimal end-to-end, float-error-exposing values | ✅ `100 × 0.10` rolls up **exactly `10.00`** (f64 control asserted to miss); the rollup column verified `is_decimal: true, is_float: false`; signed fold `0.30 → 0.10 → 0.30` returns **exactly `0.30`** |
| 2 | Tier-1 EVENT arithmetic audited empirically | ✅ probe table above — decimal-preserving and decimal-promoting; no coercion |
| 3 | WO-007 reconciliations re-run, decimal-equal | ✅ all four ladder tests green with **exact equality**, epsilons deleted; mixed-scale reconciliation `live=101.305 rolled=101.305` |
| 4 | Migration path for existing rollups | ✅ `recompute_from_source()` — resets rollup + cursor, replays the feed in decimal. Test seeds a legacy `0.30000000000000004f` rollup and asserts it is **replaced, not converted** |
| 5 | Floor + gates | ✅ isolated run: hook 0 ms, floor **25 ms**/25, realtime tax **2 ms**/2 |

## What changed

- **`decimal.rs`** (new, ~140 lines, no dependency): exact fixed-point decimal — scaled `i128` mantissa + scale. Add and negate only. **Multiplication and division are deliberately absent**: rounding policy belongs to REQ-6.2.2 (scale from currency metadata, explicit mode, defined points), and a silent default in a utility type is the wrong place to decide it.
- **`aggregates.rs`**: `Contrib` returns `Decimal`; the per-batch signed fold is exact; the emitted statement uses `dec`-suffixed literals against `(f ?? 0dec)`. No `f64` remains on any money path in the module.
- **`sync.rs`**: `Currency` → decimal DDL (the root fix above).
- **WO-007's `reconcile()` helper**: epsilon comparison replaced with decimal equality. *An epsilon in a REQ-6.2.1 reconciliation hides exactly the defect the requirement forbids* — that helper was asserting the wrong thing since WO-007.

## Migration, stated plainly

**Rollups are derived data, so the upgrade is recompute-from-source — never conversion of stored floats.** Converting would launder the error into a decimal that *looks* exact; recomputing erases it. `recompute_from_source()` resets the rollup docs and the cursor, and the next drain rebuilds every bucket from the changefeed in decimal. Cost is one full feed replay per rollup, one time, and the result is reconcilable against the source — which the (now decimal-exact) reconciliation tests assert.

Ordinary documents are a different case worth naming: their `Currency` columns change type float→decimal on the next sync. Existing stored values convert as SurrealDB sees fit; any float error already in them is *historical data*, not something this WO can honestly repair. Recomputation is only available where a value is derived.

## Findings

1. **A test caught an inconsistent `Eq`/`Ord` pair in my own decimal type** — derived `PartialEq` compares representation (`1.50` ≠ `1.5`) while `Ord` compares value, which Rust requires to agree. Equality is now numeric, matching both SurrealDB and the runtime surrogate.
2. **Two of my first assertions asserted representation, not value** (`"10"` vs `"10.00"`). SurrealDB normalises trailing zeros; the requirement is decimal *equality*. Fixed by asking the DB to compare (`amount = 10.00dec`) — which is what "decimal-equal, not epsilon-equal" actually means, in both directions.
3. **Counts became decimal too** (the `Contrib` API is uniform). Numerically exact and harmless, but worth a note: `n` in a rollup is now a decimal `11`, not an int.

## Suite state
**26 binaries green, zero failures.** Perf gates green in their own invocation per the WO-014 practice (floor 25/25, tax 2/2 — both at their limits on this substrate, worth watching). Substrate probe run before measurement; scratch databases dropped at close.

## Related
[[WO-016 Decimal Rollup Accumulation]] · [[ADR-010 Materialized Aggregates]] (amendment can lift) · [[SRS]] (REQ-6.2) · [[2026-07-25 WO-015 child-table line editor]] · [[2026-07-24 WO-007 aggregates ladder implementation]] · [[SurrealDB]]
