---
tags: [frust, work-order, desk, child-tables]
status: COMPLETED 2026-07-25 — all 6 criteria; browser line edit moved the Tier-2 rollup (+150.0 exact delta); ADR-008 A4 promissory note cashed (per-field line diff); reveal-not-clone rows (trigger untouched); 2 silent-wrong bugs fixed (envelope depth decode; decimal-as-string zeroing); f64-rollup finding ESCALATED → WO-016. → [[2026-07-25 WO-015 child-table line editor]]
created: 2026-07-25
---

# WO-015: Child-Table Line Editor (the Canonical ERP Form, Completed)

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. Governing: [[ADR-008 Data Shape]] (embedded children, storage-agnostic access), [[ADR-004 Topcoat for Desk v0]] (**revisit-trigger #1 stays armed — this is NOT the spreadsheet grid**), [[2026-07-25 WO-014 Desk v2 dynamic forms]] (the dynamics layer lines compose with).

## Scope

A basic line editor in Desk forms: add / edit / remove rows of an embedded child table, submitted as the embedded array through the existing envelope. Frappe's standard child-table form experience — **not** bulk edit, not per-cell reactivity, not copy-paste ranges. If a criterion pulls toward spreadsheet-grade, stop: that's the ADR-004 trigger, and it's a PM conversation.

## Exit Criteria

1. **Metadata-driven line rendering:** a child-table field (e.g. `lines` on a sales-invoice-shaped DocType) renders as rows from the child DocType's own field metadata — created at runtime, no recompile (the standing Tier-1 property).
2. **Row operations through the envelope:** add / edit / remove rows client-side; save submits the whole embedded array through `db_write` — hooks see the full doc (WO-007's line-diffing continues to work on the result: prove a line edit lands in the Tier-2 item rollup).
3. **Line-level dynamics compose:** WO-014 rules work *inside* rows where declared (a row's amount field warning on a money rule; a row field's `depends_on` another field in the same row). Row-scoped signals, same zero-round-trip property.
4. **Decimal per line:** line amounts render/edit as decimals; compare-never-compute holds per row; **line totals are NOT client-computed** — the displayed total comes from the server (hook-computed), refreshed on save. State this in the UI honestly (a "computed on save" affordance), don't fake it client-side.
5. **The lattice on lines:** at Submitted, rows are frozen with the rest of the form (no add/remove/edit); `allow_on_submit` granularity stays field-level, not row-level, per current metadata semantics.
6. **The canonical proof:** an invoice-with-lines DocType created at runtime → lines added in the browser → submitted → item-wise rollup reflects the lines → audit trail shows the line history (whole-doc entries per ADR-008 A4 — render the line diff presentation-side from before/after arrays, the promised computation, at least minimally).

## Escalations

Standard rules + full hygiene set. Spreadsheet-pull = stop (see Scope). Any envelope/`FLEXIBLE` surprise on nested arrays → check the [[SurrealDB]] caveats first (the `lines.*` rule), then escalate if it's new.

**Related:** [[Frust Hub]] · [[ADR-008 Data Shape]] · [[ADR-004 Topcoat for Desk v0]] · [[ADR-010 Materialized Aggregates]] · [[2026-07-25 WO-014 Desk v2 dynamic forms]]
