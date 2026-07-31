---
tags: [frust, work-order, desk, workflow, v1.1]
status: COMPLETE (2026-07-28) — buttons render from metadata for (state,role), drive the real /transition, proven end-to-end in a real browser (18 checks: create→Submit→Approve→Reject→Reopen, role-filtering, ROLE_DENIED-as-prose, dirty-guard-intact, docstatus 0→1 at the DB floor). Finding: kernel resolved missing but not EMPTY workflow_state to initial — every Desk-created doc had an empty one, so a fresh doc showed no buttons; fixed via WorkflowDef::state_or_initial. See [[2026-07-28 WO-031 desk workflow buttons]].
created: 2026-07-28
---

# WO-031: Desk Workflow Buttons (Close the c5 Overclaim)

> [!info] PM work order — next v1.1 item, chosen because it closes an **honesty debt** (a criterion marked done that the browser proved wasn't) AND makes the workflow engine actually usable by a real user. Governing: [[WO-018 Workflow Engine]] (criterion 5 corrected: engine real, Desk buttons unbuilt), [[2026-07-26 WO-022 accounting seed]] (F5), [[ADR-012 Row-Write Permission]] (clerk transitions stay at docstatus 0).

## The debt

WO-018 criterion 5 ("transition buttons render from workflow metadata for the current (state, role)") was marked done; WO-022's browser leg proved the Desk renders **no workflow buttons** — only the legacy docstatus-submit button. The workflow *engine* is real and drives via `/transition`; the Desk-affordance half was never built. The accounting seed cannot be driven through approval by a real user in the browser — only by REST. This closes that gap and the overclaim.

## Exit Criteria

1. **Buttons render from workflow metadata for the current (state, role):** a document under workflow shows exactly the transitions *this user's role* may take from *this state* — no more (a clerk sees "Submit for Approval", not "Approve"), driven by the workflow definition, no hardcoding. A document with no workflow shows the existing docstatus affordances unchanged.
2. **Clicking a button drives the real `/transition` path** — the same endpoint WO-018 proved, the same kernel judge + lattice backstop. The button is a UI affordance over the proven mechanism, never a reimplementation of the transition logic.
3. **The lattice/permission rules hold through the UI** (ADR-012): a clerk's transitions stay at docstatus 0; an illegal transition surfaces the typed `FRUST:E_WORKFLOW`/`E_DOCSTATUS` error as a user message (internals stripped, ADR-007 hygiene); a role that may not transition sees no button for it.
4. **The canonical seed flow, clicked in the browser, end to end:** clerk creates an invoice with lines → clicks "Submit for Approval" (0→0) → manager sees and clicks "Approve" (0→1) → rejection path back. The WO-022 flow that only ran via REST, now driven by a real user through the Desk. **This is the live-through-the-browser proof, per the standing "tested-seam ≠ wired" check — not a broker test.**
5. **No regression:** existing Desk tests green; the dirty-guard (WO-014 — a live tick must not discard unsaved input) still holds when a transition button is present.

## Boundaries

- Buttons render from metadata; do not hardcode the seed's specific states. A different app's workflow must render its own buttons with zero Desk code changes (the Tier-1 metadata-driven property).
- Frontend WO (`frust-desk`). No kernel changes expected; if a button needs a kernel change to work, that's a finding (the transition surface should already be complete from WO-018) — report it.

## Escalations

Standard rules. Browser proof is mandatory (criterion 4) — this WO exists *because* a broker-only proof let an overclaim stand; it does not get to close on tests alone.

**Related:** [[Frust Hub]] · [[WO-018 Workflow Engine]] · [[ADR-012 Row-Write Permission]] · [[2026-07-26 WO-022 accounting seed]] · [[Topcoat]]
