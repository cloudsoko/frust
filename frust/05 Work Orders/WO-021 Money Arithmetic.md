---
tags: [frust, work-order, money, decimal]
status: COMPLETED 2026-07-26 — all 6 criteria. mul exact (scale grows, never rounds), div_round explicit via true remainder, round() the sole primitive, no global config; half-even proven vs half-up (2.125→2.12/2.13); defined-points load-bearing (3×1.005 = 3.00 per-line vs 3.02 end-only, divergence asserted); typed overflow/div-zero via checked_mul. **Escalation resolved as CI property: DB-side (math::round) and Rust-side money math byte-equal for every seed op — `money_reconciliation.rs` reddens if an upgrade breaks it; raw `/` (implicit 28-place round) stated+avoided.** Bug caught first-run: Down rounded 2.129 up. Perf on scratch dir: submit 25–26/60, tax 0.15–0.47/2. → [[2026-07-26 WO-021 money arithmetic]]
created: 2026-07-26
---

# WO-021: Money Arithmetic (REQ-6.2.2 — decimal.rs Learns to Multiply)

> [!info] PM work order. Governing: [[SRS]] REQ-6.2.2 (the spec exists — **implement it, don't re-debate it**), [[2026-07-26 WO-016 decimal rollup accumulation]] (`decimal.rs` deliberately shipped without mul/div; this WO is the deliberate deferral coming due). The accounting seed (WO-022) is `qty × rate` — unbuildable until this lands.

## Scope

`decimal.rs` gains multiplication and division **with explicit rounding**, exactly as REQ-6.2.2 specifies. No implicit rounding anywhere; scale from currency metadata; mode explicit (default half-even); intermediates carry extended precision; rounding happens once, at defined points.

## Exit Criteria

1. **Multiply and divide exist, rounding is never implicit:** `mul`/`div` either return extended-precision unrounded results that a separate explicit `round(scale, mode)` collapses, or take the rounding contract as an argument — your design, but a caller must never get a silently-rounded result. A raw `*` that rounds to 2 places by guessing the scale is the defect this requirement forbids.
2. **Rounding mode is real, and half-even is proven against half-up:** `round(2, HalfEven)` on `2.125 → 2.12` and `2.135 → 2.14` (banker's), with a half-up control asserting the *different* answer — the WO-016 pattern (a control that proves the test bites). Scale comes from currency metadata, not a hardcode.
3. **The defined-points discipline holds on a real calc:** line = `qty × rate` rounded once; tax = `rate% × subtotal` rounded once; total = sum of rounded lines. Prove that rounding at each defined point gives the auditable answer and that rounding *only at the end* (or *at every intermediate*) gives a different, wrong one — so the discipline is demonstrably load-bearing, not decorative.
4. **The boundary guarantees still hold:** results cross the ADR-006 envelope as decimal strings (WO-014/016); a computed money value is never a float at any boundary (REQ-6.2.1); the WO-017 script catch still refuses script-mangled money. Multiply doesn't reopen the float door.
5. **Overflow/precision limits are typed, not panics:** a multiplication that exceeds the type's range fails typed (`E_MONEY_OVERFLOW` or similar), never wraps or panics — money math that silently wraps is worse than money math that stops.
6. **Floor holds:** full hygiene set, perf gates on a **dedicated scratch data-dir** (the new caveat), against the WO-018 baseline.

## Boundaries

- This is arithmetic, not a tax engine — `qty × rate`, percentage-of, allocation-divide. Tax *rules*, multi-currency conversion, and rounding-remainder-distribution across allocations are WO-022's problem if the seed needs them; note anything that pulls further rather than building it here.
- Rounding mode is per-money-field metadata; do not invent a global rounding config (REQ-6.2.2 says explicit-per-field).

## Escalations

Standard rules + full hygiene set. If v3.2.0's own decimal arithmetic (in EVENT bodies / SurrealQL) rounds differently than `decimal.rs`, that split is a finding — the Tier-1 rollup counters do arithmetic DB-side (WO-016 probed add; mul/div is new) — report before assuming they agree.

**Related:** [[Frust Hub]] · [[SRS]] (REQ-6.2.1, REQ-6.2.2) · [[2026-07-26 WO-016 decimal rollup accumulation]] · [[ADR-010 Materialized Aggregates]] · [[WO-022 Accounting Seed]]
