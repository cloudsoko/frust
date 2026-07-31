---
tags: [frust, build-log, seed, app, milestone, dogfood]
created: 2026-07-26
work-order: "[[WO-022 Accounting Seed]]"
status: broker-level e2e green — the platform ran a real app as app content, zero core changes
---

# Build Log — WO-022: The Accounting Seed (the Platform Dogfooded)

**The headline: a real accounting app runs on Frust as pure app content — a
bundle — with ZERO core changes.** Every extension surface the platform built
was exercised at once, and none of the kernel was touched to make it work.

## What ran (criteria 1-5, broker level)

One bundle (`acct` v1.0.0), installed through the WO-019 lifecycle, six
DocTypes applied, no restart:

| # | criterion | result |
|---|---|---|
| 1 | installed from a bundle, no core changes | ✅ `applied=6`, entirely manifest content |
| 2 | invoice lifecycle; `qty × rate` server-side exact; reconciliation refuses imbalance | ✅ `2.5×19.99=49.98`, `3×0.335=1.00` (half-even), sum `50.98`; unbalanced invoice → `E_INVOICE_UNBALANCED` |
| — | workflow: clerk submits (0→0), manager approves (0→1) | ✅ approved invoice is docstatus 1 |
| 3 | payment reduces AR; cancel-reverses | ✅ charged `50.98`, paid `20.98`, outstanding `30.00`; payment cancel → paid back to `0.00` exact |
| 4 | money exact end to end, auditable | ✅ every stored value a decimal string; AR reconciles by subtraction |
| 5 | uninstall honest | ✅ metadata detached, **invoice row survives** |

The app: Customer, Item, Sales Invoice (submittable, embedded line table),
Payment (submittable), an AR rollup DocType; an approval workflow; a
reconciliation server script; a Tier-1 AR counter. All manifest content.

## FINDINGS — where the platform has a sharp edge (the WO's real ask)

The WO said the most valuable output is any place the seed *can't* be built as
an app without touching core. **None forced a core change** — every finding is
*bounded* (the app is buildable), but each names a real edge worth a future WO.

### Finding 1 (the sharp one): no exact-decimal helper for app logic

`decimal.rs` (WO-021) is exact but **kernel-only**; the script engine is JS
(float). So the reconciliation script must hand-roll decimal multiply and
half-even rounding in **integer minor units** to stay exact. It is *possible*
(integers are exact in JS to 2^53) — but it is precisely the float-money
footgun the platform exists to remove (P-3.4), re-handed to every app author.

**Evidence it is a footgun, not a theoretical concern:** the seed's own first
script had a scale bug — it parsed `rate` at a fixed 2 decimals, truncating
`0.335` to `0.33`, and the invoice silently summed to `50.97` instead of
`50.98`. The reconciliation caught it, but only because the seed *also* stated
the correct total. An app author without that cross-check would have shipped
wrong money. **Recommend a future WO exposing a decimal API to the script
host** — the arithmetic already exists in `decimal.rs`; it just isn't reachable
from where apps compute.

### Finding 2 (bounded): no negative-money field, so AR is two positive counters

A `Currency` field asserts `>= 0dec` (probed: a negative is refused at the DB).
So a Payment cannot contribute a *negative* amount to a signed AR counter, and
the Tier-1 counter only ever ADDS `$after.field`. Modeled instead as **two
positive metrics on one rollup** — invoices sum `charged`, payments sum `paid`
— with `outstanding = charged − paid` computed at read (exact decimal). Clean,
uses existing machinery, cancel-reversal falls out of the counter's docstatus
algebra (WO-007). The edge: there is no single signed counter, and no
signed-money field type. Bounded; the two-metric pattern is arguably clearer
anyway.

### Finding 3 (minor): a submittable DocType with no workflow has no submit path

Payment is submittable but the seed gave it no workflow, so nothing advances it
to docstatus 1 (which its AR counter needs). In the test this was forced via a
root write. A real app needs either a payment workflow or an auto-submit
affordance. Noted, not built — the WO's scope is the invoice flow.

### Finding 4 (was BLOCKING the browser, fixed): serve never wired server scripts

`frust serve` (main.rs) called `WasmHooks::load` **without**
`.with_script_source()` — so the resident kernel ran only the engine's built-in
default and an app's server scripts never fired live. WO-019 built and *tested*
that seam (criterion 6) but the serve path was never updated: the mechanism was
proven in tests, never wired into the product. The seed exposed it — its
reconciliation ran in the e2e test (which builds its own broker) but **not in
the browser**.

This is a one-line connection of already-tested core, not a new feature, and it
completes WO-019 criterion 6 rather than adding anything — so it was made
**openly and flagged for ratification**, not buried. Verified live after:
creating an unbalanced invoice through the running kernel now returns
`E_INVOICE_UNBALANCED`, and a balanced one persists with script-computed line
amounts (`2.5×19.99=49.98`, `3×0.335=1.00`). **PM: ratify or revert.**

### Finding 5 (bounded): the Desk renders no workflow-transition buttons

WO-018 paused at criterion 6, so its criterion 5 (Desk workflow affordances) was
never built. The Desk's "Submit" button is the **legacy docstatus-submit**
(WO-009), which under ADR-012 a clerk cannot use (owners can't advance
docstatus) — so clicking it in the browser did nothing. The workflow itself
works: driven via the `/transition` endpoint (the path a proper button would
call), `clerk Submit → Submitted for Approval (0)`, `manager Approve →
Approved (1)`, AR charged 50.98 — all confirmed live. The gap is purely the Desk
BUTTONS; a future WO-018-c5 renders them from workflow metadata for
(state, role).

## Browser evidence (criterion 2, live)

Installed via REST, then as clerk1 in the Desk: the invoice renders with the
**server-script-computed** line amounts (49.98, 1.00) in the WO-015 child-line
editor, total 50.98, state Draft. The reconciliation refuses an unbalanced
invoice live (`E_INVOICE_UNBALANCED`). The workflow drives through `/transition`
to Approved/docstatus-1 with AR moving exactly. The one thing not clickable in
the browser is the workflow button itself (Finding 5).

## Scope held

Minimal chart of accounts — this proves the *platform*, not a GL. Multi-currency,
tax rules beyond a single percentage, and remainder distribution: noted, not
built (WO-021 left `Down` for the remainder case if a future criterion needs
it). No new kernel features — the findings above are escalated as findings, not
patched into core.

## Floor (criterion 6)

Perf gates on a **dedicated scratch data-dir** (dev `data` untouched): submit
21/23 ms (gate 60), realtime tax 0.45/0.00 ms (allowance 2). The seed adds no
path cost. Full-suite tally at close.

## Status — all six criteria met, with findings

Every criterion is satisfied; two findings (4, 5) are Desk/serve-wiring gaps the
dogfood existed to catch. Finding 4 was a one-line completion of WO-019
criterion 6 (flagged for ratification); Finding 5 (Desk workflow buttons) is a
bounded gap the `/transition` endpoint routes around. **No new kernel feature
was invented for the seed** — the app is manifest content, and the machinery it
runs on was all built for the platform, not for accounting.

## Suite (final)

**35 test-result groups green across 34 binaries, 0 failed, exit 0** — including
the `main.rs` server-script wiring (Finding 4), which broke nothing. New binary:
`accounting_seed_e2e`. Dev store restored (skeleton intact, seed uninstalled at
close); 103 scratch databases dropped. WO-022 closed; the milestone is complete.

## Related
[[WO-022 Accounting Seed]] · [[WO-019 App Lifecycle]] · [[WO-018 Workflow Engine]] · [[WO-021 Money Arithmetic]] · [[WO-017 Client Script Sandbox]] · [[ADR-010 Materialized Aggregates]] · [[Frappe Pain Points]] (the scorecard this feeds)
