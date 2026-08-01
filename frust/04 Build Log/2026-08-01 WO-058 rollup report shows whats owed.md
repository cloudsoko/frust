---
tags: [frust, build-log, desk, reports, money, milestone-5]
created: 2026-08-01
work-order: "[[WO-058 Rollup Report Shows Whats Owed]]"
status: DELIVERED — the AR report answers its own question. One entry per rollup, all metrics unioned, and `outstanding = charged − paid` computed with EXACT DECIMAL (scaled integers, no float anywhere). Browser-asserted: Meridian Logistics — count 2, paid 120.00, charged 300.00, **outstanding 180.00**. Two findings recorded rather than patched: `n` was stripped by the read envelope, and a live session survives a kernel restart only to fail as a 500.
---

# WO-058 — the rollup report shows what's owed

## The answer, content-asserted in the browser

| bucket | count | paid | charged | outstanding |
|---|---|---|---|---|
| Meridian Logistics | 2 | 120.00 | 300.00 | **180.00** |

Asserted as data, not pixels — the four criterion-4 values read exactly
`"2"`, `"120.00"`, `"300.00"`, `"180.00"` from the rendered table. The Reports
index now lists **one** entry: *"ar outstanding · Tier 1 · exact · from payment,
sales_invoice"*.

## What was wrong, precisely

Reports were generated **per aggregate-declaration, not per rollup**:

- the index pushed one row per declaration, so a rollup fed by two doctypes
  appeared twice, as two identical links to the same URL;
- the report page `.find()`-ed the **first** declaration and rendered only its
  metrics — which is why `paid` showed and `charged` was absent, while the
  stored row had both.

Both are now keyed on the rollup: the index dedupes and lists every feeder, and
the page collects **all** declarations targeting the rollup and unions their
metrics (order-preserving, de-duplicated). Tier is the *weakest* guarantee among
the feeders — if any feeder is worker-maintained the report says Tier-2, because
the reader must be told the weaker thing.

## Where the subtraction computes, and the finding under it

**In the Desk, on the decimal strings the kernel sent, via scaled `i128`
integers. No float type is constructed at any point.** This is presentation-
derived and writes nothing, so ADR-007's compare-never-compute is untouched —
but "presentation" is not a licence for approximation, so it is exact.

`money_sub(a, b, scale)`:
- parses sign / integer / fraction, refuses anything that isn't a plain decimal
  (returns `None`, so the cell shows nothing rather than a wrong number);
- **refuses over-scale input rather than rounding it** — the same posture
  `pad_money` takes, because silently dropping a place inside a money
  subtraction is the exact defect class this guards;
- subtracts as integers and formats back at scale, negatives included
  (overpayment is a real accounting state).

Tests pin the classic float traps that an `f64` implementation fails:
`0.30 − 0.10 = 0.20`, `0.03 − 0.01 = 0.02`, `8.45 − 4.35 = 4.10`, and
`99999999999.99 − 0.01` exact past f64's 2^53 mantissa.

### FINDING — this is a third decimal implementation

The Desk has **no shared decimal**. `decimal.rs` lives in the kernel and is
compiled verbatim into the Boa sandbox (WO-030), so `money_sub` is a **third**
implementation of decimal arithmetic in this codebase — and WO-030's own lesson
was *three hosts must give one answer*.

It is deliberately the smallest thing that can be correct (subtraction, one
scale, nothing else), and it is tested. But the honest long-term home is either
a **kernel report path** where `decimal.rs` already lives, or an exposed decimal
the Desk can share. **That is a PM decision, not something to settle inside a
Desk WO** — recorded here rather than quietly accreted.

## FINDING — `count` was blank because the read envelope stripped `n`

WO-056 reported the `count` column as a rendering gap. **It was not.** The Desk
asked for `row["n"]` correctly; the kernel never sent it:

```
stored:      { k, charged, paid, n: 2 }
read door:   { k, charged, paid }          ← n stripped
```

`ar_outstanding` declared only `k, charged, paid`, and WO-009's envelope bounds
read output to declared fields. The rollup's `n` counter is maintained by the
ADR-010 EVENT but was never declared on the rollup DocType, so it was invisible
through the one door the Desk is allowed to use.

**Remedied as app content, not kernel source:** `n` is now declared on the
`ar_outstanding` DocType (a metadata write to the dev store, stated below). No
kernel change was needed or made — the envelope behaved exactly as designed;
the app under-declared its own rollup.

*Worth carrying forward:* every ADR-010 rollup has an `n`, and any rollup whose
DocType omits it will have the same invisible-count problem. Whether rollup
DocTypes should declare `n` automatically at sync is a real question for the
aggregates engine — **not touched here**, per the boundary.

## FINDING — a live session survives a restart only to fail as a 500

While proving this, the report returned *"Something went wrong on the server"*
under the browser's existing session, while an identical request with a
**freshly minted token succeeded**. Logging out and back in fixed it.

The failing call logs `kind:"session"` + `E_DB` → HTTP 500. This is WO-008's
banked caveat — *DEFINE ACCESS redefine = JWT rotation* — reaching the user
layer: boot re-applies the meta DDL, the record-access secret is re-issued, and
every session holding a JWT minted under the old secret starts failing at the
database. The kernel's own session row is still valid, so the user is **not**
sent to the login page; they get a 500 that reads like a server fault.

Not fixed here (kernel/session behaviour, outside a Desk WO). The right answer
is probably to recognise the rotated-JWT refusal and answer **401 "please log in
again"** instead of 500 — the same shape as WO-055's G1, one layer down.

## Scope held

- **Zero kernel source changed by this WO.** (`frust-kernel/kernel/src/` shows
  only the concurrent session's `recovery.rs`/`tenancy.rs` work, which is not
  mine and which I left alone.)
- **AR *aging* is out**, as ruled — it needs per-invoice dates, which the
  rollup does not carry. The gap stands in WO-056's list.
- `outstanding` is derived only when a rollup has **both** `charged` and `paid`,
  so it is a convention the accounting rollup satisfies rather than a column
  invented for every rollup. A metadata vocabulary for derived columns would be
  the general answer, and is a feature, not this WO.

## Verification

Desk tests **5/5** (including `money_sub_is_exact_and_never_float`); the Reports
index shows one entry naming both feeders; the report content-asserts
2 / 120.00 / 300.00 / **180.00**. Live through `frust serve` + browser.
Screenshot: `frust-e2e/wo058-ar-final.png`.

**No commits made**, per instruction — the working tree contains a concurrent
session's in-flight work and committing would braid the two together.

## Dev-store mutation

`doctype:ar_outstanding` gained a declared field `n` (Int). Nothing else.

## Related
[[WO-058 Rollup Report Shows Whats Owed]] · [[2026-08-01 WO-056 complete the dogfood]] (where the gap was found) ·
[[ADR-010 Aggregate Ladder]] · [[ADR-007]] (money display ruling) · [[WO-030]] (three hosts, one answer) ·
[[WO-008]] (the JWT-rotation caveat this surfaced)
