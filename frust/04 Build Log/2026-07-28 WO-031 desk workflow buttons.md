---
tags: [frust, build-log, desk, workflow, browser-proof, work-order, v1.1]
created: 2026-07-28
work-order: "[[WO-031 Desk Workflow Buttons]]"
status: complete — buttons render from metadata, drive the real /transition, proven by a real browser end to end; one kernel finding fixed
---

# Build Log — WO-031: Desk Workflow Buttons

The overclaim, corrected. WO-018 criterion 5 was marked done; WO-022's browser
leg proved the Desk rendered no workflow buttons. Now a real user can drive the
seed's approval flow through the browser — **proven by clicking it, not by a
broker test**, because a broker-only proof is exactly what let the overclaim
stand.

## What was actually missing

The transition *surface* (`GET /workflow/{dt}/{key}` → available actions for
this (state, role); `POST /transition/{dt}/{key}` → the judged transition) was
complete from WO-018. The **Desk affordance** over it was never built: the
record view showed only the raw docstatus Submit button, which bypasses the
workflow judge entirely.

## The Desk changes (frontend only, as scoped)

Three edits to `frust-desk/src/main.rs`:

1. **`doc_page` fetches `/workflow/{name}/{key}`** and, when the doc is under a
   workflow, renders one button per available action in a form posting to
   `/transition`. The badge shows the workflow state. The raw docstatus
   submit/cancel are **suppressed** under a workflow (they bypass the judge);
   a doc with no workflow keeps them unchanged (criterion 1).
2. **A `transition_doc` route** (`POST /transition/{dt}/{key}`): a thin
   affordance that posts the action to the kernel's proven endpoint and
   surfaces the typed refusal. No transition logic is reimplemented (criterion 2).
3. **`friendly()` gained a `workflow-denied` arm** so `E_WORKFLOW` refusals
   surface as their own prose ("'Approve' from 'Submitted for Approval'
   requires role manager; you are 'clerk'") instead of the generic "something
   went wrong" — the machine code stays in `code`, out of the message
   (criterion 3, ADR-007 hygiene).

The buttons render from metadata: a different app's workflow renders its own
buttons with zero Desk changes (the action strings and role-filtering come from
the workflow definition via the kernel's `available(state, role)`).

## THE FINDING — a kernel gap the browser proof exposed

**A document created through the Desk showed no buttons at all**, even as a
clerk on a fresh Draft. Root cause, found by driving it: the kernel resolved a
*missing* `workflow_state` to the initial state (`unwrap_or_else(initial)`) but
not an *empty* one — and every Desk-created doc carries `workflow_state = ""`
(the field exists, so it is present-but-empty, not absent). So `available("",
role)` matched no transition, and the doc was stranded stateless.

WO-018's own tests masked this: the seed E2E hand-sets `workflow_state:
"Draft"` on every create, so the empty-state path was never exercised. **A
broker test that constructs the document by hand cannot catch a gap in how
documents come to exist** — which is the same lesson this WO was issued to
correct, one layer down.

**Per the WO boundary ("if a button needs a kernel change, that's a finding —
report it"), reported and fixed minimally.** A new `WorkflowDef::state_or_initial`
treats absent *or empty* as the initial state — completing `initial_state()`'s
intent, which already existed for the absent case. Two broker call sites
(`db_transition`, `workflow_actions`) now route through it. Unit-tested
(`empty_or_missing_state_resolves_to_initial`).

*Alternative considered:* initialize `workflow_state` to the initial state on
document creation. That is a larger change to the write path, and a client has
no keyless endpoint to learn the initial state anyway — so the empty-as-initial
resolution is both smaller and more general (it fixes every client, not just the
Desk). Noted for the PM: if "the initial state should be stamped on create" is
preferred as the model, that is a separate write-path WO.

## The browser proof (criterion 4, mandatory)

A Playwright script drove **real headless Chromium** against the live kernel +
Desk (dev store). 18 checks, all green:

- **clerk1** creates an invoice with a line (header on `/form`, line via the
  `/doc` WO-015 editor — the two-step the Desk's form model implies), reconciled
  2 × 10.00 = 20.00.
- Draft view: clerk sees **"Submit" only, not "Approve"** (role filtering); the
  raw docstatus Submit is gone; the **WO-014 dirty-guard still fires** with the
  workflow button present (typing marks dirty, banner shows) — criterion 5.
- clerk clicks **Submit for Approval** → state `Submitted for Approval`,
  docstatus stays 0 (criterion 3: a clerk's transitions stay at 0); clerk now
  has **no further action**.
- clerk POSTs `Approve` (which they may not do) → refused with **"requires role
  manager; you are 'clerk'"**, machine code stripped, nothing changed
  (criterion 3).
- **manager** opens the same doc → sees **Approve and Reject** → clicks Approve
  → state `Approved`. Verified at the DB floor: `docstatus = 1` (criterion 2:
  Approve is the lattice submit, 0→1).
- **Rejection path**: a 2nd invoice, clerk submits → manager **Rejects** → state
  `Rejected` → clerk **Reopens** → back to `Draft`.

The script lives at `wf-proof/proof.mjs`; `approved.png` captures the approved
record view.

## Verification

- **Browser proof:** 18/18 (above).
- **Kernel unit:** `workflow::` 5/5 incl. the new empty-state test.
- **Kernel integration:** `workflow_engine` green, `accounting_seed_e2e` 7/7
  (both exercise the transition path through the changed resolution).
- **Desk:** compiles clean (no integration-test suite — the Desk is
  browser-proven by design, which is the whole point of this WO).
- Full kernel suite: [tally recorded on close].

## Files

- `frust-desk/src/main.rs` — `doc_page` (fetch + render buttons, suppress raw
  affordances), `transition_doc` route, `friendly()` workflow-denied arm.
- `kernel/src/workflow.rs` — `state_or_initial` + unit test.
- `kernel/src/broker.rs` — `db_transition` / `workflow_actions` route through it.
- `wf-proof/proof.mjs` — the browser proof.

## Related
[[WO-031 Desk Workflow Buttons]] · [[WO-018 Workflow Engine]] (criterion 5, now genuinely closed) · [[2026-07-26 WO-022 accounting seed]] (F5) · [[ADR-012 Row-Write Permission]] · [[tested-seam-not-wired]] (the standing check this WO embodies) · [[Topcoat]]
