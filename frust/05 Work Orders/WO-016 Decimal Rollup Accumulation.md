---
tags: [frust, work-order, money, aggregates]
status: COMPLETED 2026-07-26 — EVENT decimal arithmetic clean (no coercion, no pillar finding); ROOT CAUSE WIDER: Currency was TYPE float in every DocType (fixed at the mapping); exact reconciliation everywhere, epsilons deleted; recompute-don't-launder migration proven; decimal.rs deliberately has no mul/div (REQ-6.2.2's business). Watch: gates at-limit (25/25, 2/2). → [[2026-07-26 WO-016 decimal rollup accumulation]]
created: 2026-07-25
---

# WO-016: Decimal Rollup Accumulation (REQ-6.2.1 Inside the Aggregates Layer)

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. This is WO-015's escalation, taken ahead of the sandbox: money correctness outranks extensibility. Governing: [[ADR-010 Materialized Aggregates]] (amendment pending), [[SRS]] REQ-6.2.

## The finding

Tier-2 rollup columns and delta accumulators are `f64` — they predate the Decimal surrogate. WO-015's parse fix stopped the silent-zero; it did not stop float money. REQ-6.2.1 says a float representation of money crossing any boundary is a defect — the rollup *is* a boundary (it's a DocType, read through the contract).

## Exit Criteria

1. **Tier-2 accumulation is decimal end-to-end:** the differ's per-line deltas, the accumulator, the rollup column, and the cursor-transaction write all carry decimal — no `f64` on any money path. The WO-015 `+150.0` scenario re-proven with a value that *exposes* float error (e.g. `0.1`-family amounts summed at volume: assert the rollup equals the exact decimal sum, not the nearest float).
2. **Tier-1 audited the same way:** EVENT counters (`monthly`, AR outstanding) — SurrealQL-side arithmetic on decimal-typed fields; verify the DDL's `+=` path holds decimal semantics (empirical first-exercise rule: prove what SurrealDB does with `decimal + decimal` in an EVENT, don't assume). If v3.2.0 EVENT arithmetic degrades decimals to float, that's a pillar finding — escalate before designing around.
3. **Reconciliation re-run:** the WO-007 full-scan reconciliations re-executed post-conversion — rollups == live aggregates *exactly* (decimal-equal, not epsilon-equal — the whole point).
4. **Migration for existing rollups:** stored f64 rollup docs get a stated upgrade path (recompute-from-source via backfill is acceptable and probably right — rollups are derived data; say so rather than converting floats).
5. **Floor + gates:** full hygiene set; both submit gates and the realtime tax gate green.

## Escalations

Standard rules. Criterion 2's EVENT-arithmetic probe is the risky unknown — any silent decimal→float coercion in the DB = pillar finding, stop and report (it would also fire the ADR-002 taxonomy question).

**Related:** [[Frust Hub]] · [[ADR-010 Materialized Aggregates]] · [[SRS]] (REQ-6.2) · [[2026-07-25 WO-015 child-table line editor]] · [[2026-07-24 WO-007 aggregates ladder implementation]]
