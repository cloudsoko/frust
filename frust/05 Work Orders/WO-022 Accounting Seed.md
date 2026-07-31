---
tags: [frust, work-order, seed, app, milestone]
status: COMPLETED 2026-07-26 (formal close on final no-regression tally) — the platform ran a real accounting app as pure bundle content, ZERO invented kernel features. Proven 3 ways: broker e2e (green), live through `frust serve` (REST — reconciliation `E_INVOICE_UNBALANCED` fired, `2.5×19.99=49.98` exact), browser (child-line editor shows script-computed amounts). Full lifecycle: clerk submit 0→0, manager approve 0→1, AR charged/paid/outstanding exact decimal, cancel-reversal to 0.00, uninstall detaches metadata + retains data. **5 findings (the real output, none inventing a feature):** (1) no exact-decimal helper for app scripts — reconciliation hand-rolls integer-minor math, *my own first version had a scale bug* (0.335→0.33) → post-v1.0 backlog (expose decimal.rs to script host), feeds scorecard P-3.4; (2) Currency asserts ≥0 so AR = two positive counters (charged/paid), subtract at read — bounded; (3) submittable+no-workflow has no submit path — minor; (4) `frust serve` never wired `with_script_source` — RATIFIED as completing WO-019 c6; (5) Desk renders no workflow buttons — WO-018 c5 overclaim, → backlog. Perf on scratch dir: submit 21–23/60, tax 0.00–0.45/2. → [[2026-07-26 WO-022 accounting seed]]
created: 2026-07-26
---

# WO-022: The Accounting Seed (the Platform Dogfooded)

> [!info] PM work order — the milestone's last build. Governing: [[2026-07-26 WO-019 criterion 7 the demo app end-to-end]] (the lifecycle it ships through), [[WO-018 Workflow Engine]] (approval flow), [[WO-021 Money Arithmetic]] (`qty × rate`), [[ADR-012 Row-Write Permission]] (clerk-transitions-stay-at-0). **This is the proof Frust is a platform, not a framework: a real app built AS AN APP, in a bundle, running on machinery none of which was written for it.**

## Scope

A minimal but real double-entry-flavored accounting app, shipped as an installable bundle (WO-019 manifest), exercising every extension surface the platform built. Small enough to finish, real enough that "it's an app, not a core feature" is a fact — the ERPNext-seed moment.

## The app (as manifest content, not core)

- **DocTypes:** Customer, Item, Sales Invoice (submittable, with child line table), Payment (submittable). Invoice lines: `qty × rate` per line via WO-021, rounded at the defined point; invoice total from rounded lines.
- **Workflow:** Sales Invoice approval — `Draft → Submitted for Approval` (clerk, docstatus 0→0 per ADR-012) → `Approved` (manager, 0→1). Rejection path back.
- **Server script:** a validate that refuses an invoice whose lines don't sum to its stated total (money reconciliation as a rule — exercises WO-017 delivery + WO-021 arithmetic + the decimal catch together).
- **Aggregate:** AR outstanding per customer (Tier-1 EVENT counter, WO-007/016 — decimal, cancel-reversal on Payment).
- **Route (optional if it earns it):** a customer-statement read, proving REQ-2.2.2 in a real app.

## Exit Criteria

1. **Installed from a bundle through the Desk**, dry-run shown, gate honored — no core changes, no restart. The app is entirely manifest content.
2. **The invoice lifecycle clicked in the browser:** clerk creates an invoice with lines (`qty × rate` computed server-side, exact decimal), submits it for approval (0→0), a manager approves (0→1); the reconciliation server script refuses a deliberately-unbalanced invoice with its typed error.
3. **A Payment reduces AR outstanding exactly** — the Tier-1 counter moves by the decimal amount, cancel-reverses on Payment cancel (WO-007 signed-contribution algebra, WO-016 decimal), reconciled full-scan exact.
4. **The money is correct end to end and auditable:** every stored money value is a decimal string, line/tax/total rounded at defined points, the numbers reconcile whether read through the report or recomputed — and the changefeed shows the invoice's full history.
5. **Disable/enable/uninstall behave** per WO-019 (metadata detaches, data remains, the honest-uninstall sentence).
6. **Floor holds** on a dedicated scratch dir; full hygiene set.

## Boundaries

- Minimal chart of accounts — this proves the *platform*, not a complete GL. Multi-currency, tax rules beyond a single percentage, and remainder distribution are noted-not-built unless a criterion genuinely needs them (WO-021 left `Down` for exactly the remainder case if it arises).
- No new kernel features — if the seed *needs* a kernel change to work, that's a finding (a gap in the platform), escalate it rather than quietly adding core code to make the demo pass. The seed's job is to find those, not hide them.

## Escalations

Standard rules + full hygiene set. **The most valuable output is any place the seed can't be built as an app without touching core** — that's the platform's real boundary, and naming it is worth more than a clean demo.

**Related:** [[Frust Hub]] · [[WO-019 App Lifecycle]] · [[WO-018 Workflow Engine]] · [[WO-021 Money Arithmetic]] · [[ADR-010 Materialized Aggregates]] · [[Frappe Pain Points]] (the scorecard this feeds)
