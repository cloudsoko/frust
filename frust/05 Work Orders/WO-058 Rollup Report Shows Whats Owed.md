---
tags: [frust, work-order, desk, reports, money, milestone-5]
status: QUEUED (2026-08-01) — activates when WO-057 closes. The headline application gap from the WO-056 survey.
created: 2026-08-01
---

# WO-058: The Rollup Report — Show What's Owed

## Why

WO-056's sharpest finding: **the accounting app's headline report can't answer its own question.** The Reports page lists "ar outstanding" **twice** (once per aggregate-declaration), and each rendered report shows only that declaration's single metric — so `paid` shows, `charged` is **absent**, `count`/`n` blank — while the data is stored correctly (`charged 300, paid 120, n 2`). Root cause: reports are generated **per aggregate-declaration, not per rollup.** A rollup fed by two source doctypes (invoice→charged, payment→paid) produces two report entries, each half-blind. The fix is Desk-layer report rendering; the data is all there.

## Exit criteria

1. **One entry per rollup, not per declaration** — the Reports list dedups: one link per target rollup doctype (`ar_outstanding` appears once).
2. **The report unions all the rollup's metrics** — `charged` AND `paid` AND `count`/`n`, per the rollup's key (per customer), not one metric in isolation.
3. **The derived answer the report exists for: `outstanding = charged − paid`** — and this is the money-safety point that must be got right: **exact decimal, never float.** Name explicitly *where* the subtraction happens (Desk-side with exact decimal, or a kernel report path where `decimal.rs` lives) and record it — a financial report doing float subtraction is the precise money-defect the project killed elsewhere (WO-016/021/030). This is display-derived, not a stored write, so ADR-007's compare-never-compute is not violated by a *presentation* subtraction — but it must be exact, and if the Desk lacks exact-decimal subtraction, that's a finding (does the report compute in the Desk or the kernel?), not a float shortcut.
4. **Content-assert in the browser against stored values:** `charged 300.00`, `paid 120.00`, `outstanding 180.00`, `count 2` — the report answers *"Meridian owes 180.00,"* padded per the money-display ruling.
5. **Desk-local** (`frust_ui.rs/.css`); **zero kernel source changed** unless a finding forces it — then STOP and report (WO-022 rule).
6. Regression green; browser-proven live through `frust serve`.

## Boundaries

- Fix the rollup-report *rendering*; do **not** redesign the aggregates/rollup engine (ADR-010).
- **Full AR *aging* (bucketed by invoice age) is OUT** — it needs per-invoice dates and is its own feature; scope this to the *outstanding* report (charged/paid/outstanding/count per customer). Note aging as a future gap if the dates aren't available.
- If the report page genuinely can't tell which declarations feed one rollup from existing metadata, that's a **finding about rollup-metadata exposure** → report it, don't guess a mapping.

## Exit

The rollup report answers its own question — what each customer owes, exact to the cent, one entry per rollup — browser-proven. Then the next application-completeness order (New-form child-table editor + refusal-preserves-lines, or naming series, per the PM's sequencing of the WO-056 gap list).
