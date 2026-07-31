---
tags: [frust, build-log, money, decimal, work-order]
created: 2026-07-26
work-order: "[[WO-021 Money Arithmetic]]"
status: all 6 criteria done — WO-021 complete; the seed (WO-022) is now buildable
---

# Build Log — WO-021: Money Arithmetic

`decimal.rs` learned to multiply and divide, with rounding that is never
implicit — the WO-016 deferral, now implemented to spec (REQ-6.2.2).

## The shape (criterion 1 — rounding is never implicit)

Three primitives, and no way to get a silently-rounded result:

- **`mul` is EXACT.** The product scale is the sum of the input scales;
  `1.25 × 1.25 = 1.5625`, scale 4. Nothing rounds. The caller `round`s at a
  defined point. A `*` that guessed a scale and rounded to it is the defect the
  requirement forbids — so `mul` simply never rounds.
- **`div_round` takes the rounding contract as an argument** (`scale`, `mode`),
  because an unrounded quotient does not terminate (`1/3`). It rounds
  **exactly**: the quotient and the TRUE remainder are computed at the target
  scale, and the remainder makes the decision — no truncate-at-guard-digits
  approximation. An exact half from division (`1/8 = 0.125`) is detected as a
  half.
- **`round(scale, mode)`** is the single rounding primitive. Every rounded
  money value in the system passes through it.

Mode is explicit per money field (default half-even); scale comes from currency
metadata (the caller passes it). **No global rounding config** — REQ-6.2.2 says
explicit-per-field, so there is nowhere to set one.

## Half-even proven against half-up (criterion 2)

`2.125` is the discriminating value, and the control is the point:

| input | HalfEven | HalfUp |
|---|---|---|
| 2.125 | **2.12** (to even) | **2.13** (away) |
| 2.135 | 2.14 | 2.14 |

The test asserts the two modes **disagree** on `2.125` — without that control a
rounding test that passed under both modes would prove nothing (the WO-016
pattern). Also proven: `0.5→0`, `1.5→2`, `2.5→2` (banker's, to even), negatives
symmetric about zero, and `Down` truncating toward zero unconditionally.

## Defined-points discipline, shown load-bearing (criterion 3)

Three lines of `qty 1 × rate 1.005`:

- **Defined points** — each line rounds to `1.00`, sum = **3.00** (matches the
  three `1.00` line items a human sees on the invoice: auditable).
- **Round only at the end** — keep lines exact (`1.005`), sum to `3.015`, round
  once = **3.02**.

The test asserts these **disagree** (3.00 ≠ 3.02). That is what turns "round at
defined points" from a doc-comment into a proof: rounding at the wrong point
gives a different, wrong, un-auditable number. Full `line → tax → total` chain
each rounded once: subtotal 3.00, tax (15%) 0.45, total 3.45.

## Overflow and div-by-zero are typed (criterion 5)

`~1e38 × ~1e38` returns `MoneyError::Overflow` (via `checked_mul`), not a wrap;
`x / 0` returns `MoneyError::DivByZero`. Money math that wraps is worse than
money math that stops. A real product (`12345.67 × 100`) is `Ok` — the guard
doesn't fire on honest values.

**Bug caught by the tests, worth recording:** the first `apply_rounding` handled
`Down` only in the exact-half branch, so `2.129` under `Down` rounded *up* to
`2.13`. The `down_truncates` test caught it immediately — `Down` now truncates
unconditionally, before any half comparison.

## ESCALATION — resolved: DB-side and Rust-side money math reconcile

The WO flagged this as the likely escalation: the Tier-1 rollup counters do
arithmetic **DB-side**, WO-016 only probed `add`, and mul/div is new territory.
Probed on v3.2.0, then asserted as a CI property (`money_reconciliation.rs`):

| operation | SurrealDB | decimal.rs | verdict |
|---|---|---|---|
| `1.25 * 1.25` | `1.5625` | `1.5625` | **agree** (exact both) |
| `3 * 0.335` | `1.005` | `1.005` | **agree** |
| `math::round` half-even | `2.125→2.12`, `2.135→2.14`, `0.5→0`, `2.5→2`, `-2.125→-2.12` | identical | **agree on every discriminating case** |
| raw `/` | `1/3 = 0.3333…` (**~28 places, implicit round**) | refuses — `div_round` needs an explicit scale | **DIVERGES — avoided** |

**The finding, stated rather than papered over:** SurrealDB's raw `/` rounds
implicitly at ~28 places. `decimal.rs` will not produce an unrounded quotient
at all. So the reconciliation holds **provided DB-side division rounds
explicitly** (`math::round(x * 10^n) / 10^n`, where the only `/` is by an exact
power of ten on an integral value) and never leans on raw `/`. For the
operations the seed actually uses — `qty × rate`, `rate% × subtotal`, half-even
rounding — the two sides are byte-equal, and the test will turn RED if a future
SurrealDB upgrade changes that. The seed's numbers reconcile exactly whether
computed in Rust or in an EVENT.

## Scope held

Arithmetic only — `qty × rate`, percentage-of (mul by a decimal rate),
allocation-divide. **Pulled further, noted not built:** tax *rules*,
multi-currency conversion, and rounding-remainder distribution across
allocations (the `Down` mode exists precisely so a remainder can be distributed
separately rather than silently absorbed — WO-022's problem if the seed needs
it).

## Suite + floor (criterion 6)

**33 kernel binaries green** (the two new: `decimal` unit tests +
`money_reconciliation`); the 34th, `perf_gates`, is green on a fresh store and
flaps only on a churned one — the standing substrate caveat, not a WO-021
regression.

Perf gates on a **dedicated scratch data-dir** per the WO-020 caveat (dev
`data` never touched): submit 25/26 ms (gate 60), realtime tax 0.47/0.15 ms
(allowance 2). The arithmetic is pure-Rust integer math on the mantissa — no
measurable path cost. Dev store restored (6 rows); 91 scratch databases dropped
at close.

## Related
[[WO-021 Money Arithmetic]] · [[2026-07-26 WO-016 decimal rollup accumulation]] · [[SRS]] (REQ-6.2.1, REQ-6.2.2) · [[ADR-010 Materialized Aggregates]] · [[SurrealDB]] (raw `/` implicit-precision finding) · [[WO-022 Accounting Seed]]
