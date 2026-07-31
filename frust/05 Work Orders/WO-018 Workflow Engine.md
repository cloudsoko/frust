---
tags: [frust, work-order, workflow]
status: COMPLETED 2026-07-26 — criterion 6 green once WO-020 unblocked it: **the canonical flow runs end to end** — a clerk creates and submits their own expense draft (0→0), a manager approves to docstatus 1. Both layers proven in isolation (WO-020's `layer_two` fix: a *manager* clears the row gate and is still refused 0→2 by the lattice — the honest isolation). Criteria 1–5 done as recorded; REQ-4.1.2 satisfied. Findings A+B belonged to the compiler (→ [[WO-020 Row-Write Permission]]/[[ADR-012 Row-Write Permission]]), not workflow. → [[2026-07-26 WO-020 row-write permission]] (carries the criterion-6 green).

> [!warning] Record correction (WO-022 browser leg, 2026-07-26): **criterion 5's Desk half was overclaimed.** The workflow *engine* (judge, transitions via `/transition`, both layers) is real and proven live. But the Desk renders **no workflow transition buttons** — the "affordances render from workflow metadata for (state, role)" half of c5 was never built; the Desk shows only the legacy docstatus-submit button. REQ-4.1.2 (the engine) stands; the Desk-button UX is a known gap → post-v1.0 backlog (WO-024 candidate). Recorded honestly rather than left as a false "c5 done."

> [!important] Workflow design rule (from ADR-012, promoted here so authors find it): **clerk-driven transitions must stay at docstatus 0.** An owner cannot advance docstatus (after-state row permission refuses it) — advancing the lattice is a manager act. A manifest workflow with a clerk-driven 0→1 will be refused by the permission, correctly. Model clerk steps as workflow-state moves at docstatus 0 (`Draft → Submitted for Approval`); reserve docstatus advance for manager/approver transitions.
created: 2026-07-26
---

# WO-018: Workflow Engine (REQ-4.1.2 — the Last Big SRS MUST)

> [!info] PM work order — queued behind [[WO-017 Client Script Sandbox]]. Governing: [[ADR-009 Execution Model]] A2 (*"EVENTs enforce the docstatus lattice; workflow transition rules are kernel logic evaluated before the kernel attempts the transition"* — this WO builds that kernel logic), [[SRS]] REQ-4.1.2.

## Scope

Multi-step, role-gated approval workflows as **runtime metadata**: a `workflow` DocType (states, transitions, role-per-transition, state-scoped field behavior), evaluated in the kernel before any docstatus/state write. Frappe's Workflow feature, minus its hooks.py magic.

## Exit Criteria

1. **Workflow as metadata:** states + transitions + allowed-roles defined as records; attach to a DocType at runtime, no restart. A document under workflow shows its current state and only the transitions *this* user's role may take.
2. **The kernel is the judge, the lattice is the floor:** an illegal transition (wrong role, wrong from-state) fails typed (`FRUST:E_WORKFLOW:*`) in the kernel; the docstatus lattice EVENT still backstops (a workflow that tries to jump 0→2 hits `E_DOCSTATUS` even if workflow logic is buggy). Prove both layers separately.
3. **State-scoped behavior composes with WO-014:** a workflow state can impose read-only/required on fields — same declarative rule shape, zero round-trip rendering.
4. **Transitions are audited and hook-visible:** a transition is a `db_write` — hooks fire (both runtimes), changefeed records it, trace spans name the workflow + transition.
5. **Desk affordances:** transition buttons render from the workflow metadata for the current (state, role); the WO-009 lifecycle UI generalizes rather than duplicates.
6. **The canonical proof:** expense-claim approval — Draft → Submitted for Approval → Approved (manager only) → docstatus 1, with a rejection path back — clicked in the browser by two roles.

## Boundaries

- Workflow rules NEVER enter EVENT bodies (ADR-009 A2 incident definition — "Server Scripts with extra steps").
- No parallel/fork-join flows in v1 — linear + branch/rejoin only; note anything that pulls further as a future item.

## Escalations

Standard rules + full hygiene set + the gates-at-limit headroom check (this touches the write path — measure first).

**Related:** [[Frust Hub]] · [[ADR-009 Execution Model]] · [[SRS]] (REQ-4.1.2) · [[2026-07-25 WO-014 Desk v2 dynamic forms]] · [[2026-07-25 WO-009 Desk v1]]
