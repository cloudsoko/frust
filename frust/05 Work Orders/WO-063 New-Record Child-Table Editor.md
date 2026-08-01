---
tags: [frust, work-order, desk, forms, milestone-5]
status: ACTIVE (2026-08-01) — the proceeding app-completeness order after WO-058. Desk-local.
created: 2026-08-01
---

# WO-063: New-Record Child-Table Editor

## Why

WO-056's next-sharpest gaps, both in the create-an-invoice flow: **(a) you can't create an invoice WITH its lines in one pass** on the New form (lines are only addable on the record page *after* create), and **(b) a validation refusal DISCARDS the typed lines** — the user retypes everything. Both are core-interaction friction; this completes "make a complete invoice," the most fundamental thing a person does in this app.

## Gates (exit criteria)

1. **The New-record form renders the child-table line editor** (add / edit / remove rows) for a doctype with a child table — the **reveal-not-clone** row mechanism WO-015 built for the record page, now on the New form. Create the parent **and its lines in ONE submit**.
2. **A validation refusal PRESERVES typed input** — lines and field values survive; the user sees the error *and* their data, never an emptied form (WO-056: "a refusal discards typed lines"). The dirty-guard/preserve-input discipline.
3. **Money stays server-computed** (WO-021), shown "computed on save"; any client-side money handling is **exact-decimal, no float** (reuse WO-058's scaled-i128 posture — and note the pending decimal-consolidation finding below; do NOT add a *fourth* decimal path, reuse WO-058's).
4. **Live browser proof:** create a 2-line invoice in one pass; refuse an unbalanced one and see the lines **survive**; submit a valid one → it persists with its lines.
5. **Desk-local, zero kernel-correctness risk**; regression green — WO-015 record-page line editor, WO-031 workflow, WO-058 report all still work (assert the behavior, not just that pages render).

## Boundaries

- **Reveal-not-clone rows** — ADR-004 revisit-trigger #1 (spreadsheet/grid) STAYS ARMED; this is row editing, not a grid. Stop and escalate if it drifts toward per-cell reactivity / bulk / copy-paste.
- `frust_ui.rs/.css` Desk-local.

## Coordination (contaminated tree)

- `frust-desk/src/main.rs` is being edited by concurrent session(s) — WO-058 saw it move ~1700 lines mid-command. **Commit ONLY your files; never `git add -A`.** Verify your work survives after each build; a compile error at a line you never touched is another session's, not yours.

## Escalation

- If preserving typed input on refusal needs a kernel change, report it — it shouldn't (it's client-side form state).
