---
tags: [frust, work-order, print, desk, milestone-4]
status: COMPLETE (2026-07-31) — all 6 criteria met, **browser-proven 24/24** on the real Desk (`pnpm print`). `/print/{doctype}/{key}` is a GENERIC metadata-driven read-only document — no per-doctype code, scalars from `dt.fields`, line columns from the child DocType's own metadata (ADR-004 discipline), so a runtime-created DocType prints with no recompile. Reached from a Print affordance on the record page; WO-045's `@media print` applies; Ctrl+P yields the invoice-quality page. **Criterion 4 asserted the spike's way — ABSENT: `customer *` · `(computed on save)` · fieldset legend · any `<input>` · any `<form>` · `+ Add row` · `remove` column; PRESENT: customer · record name · line item · workflow state · every line column · amounts.** **Criterion 3, the money ruling built and landed in [[ADR-007 Tier-2 Script Architecture]]:** stored `"15"` displays **`15.00`**; over-scale SURFACES (`1.005` stays `1.005`, never rounded — money is stored AT scale so extra places are a defect to show, per the ruling); non-decimals pass through; unit-pinned incl. a property check that padding never alters the numeric value; **asserted in-browser that the STORED value is unchanged** (presentation-only is a claim about writes, so the test checks writes). Scale is 2 everywhere and NAMED as a gap — metadata has no `precision`. **Criterion 5, one door:** reads through the kernel under the caller's own session (the identical call the record page makes), so permissions apply by construction — manager 33 rows / clerk1 29; a clerk CAN print what they can read, and a manager-only record is NOT printable by the clerk, **with a vacuity guard** that fails loudly if no such record exists. **FINDING — my own WO-045 CSS had a live defect: an unscoped `table td:last-child` (hiding the editor's `remove` column) would have eaten the AMOUNT column of this very page.** It shipped one WO ago and was green because the only table it had met was the editor's; now scoped to `form table …` and pinned by an assertion that every line column survives print. WO-042's prediction recurring exactly: the Rust↔CSS seam has no type system and an over-broad selector fails silently — so I also ran WO-042's suggested guard (custom properties referenced-vs-defined: **71 defined / 66 referenced / 0 undefined**), which caught one of mine (`--fui-outline` doesn't exist) before it shipped. **THREE GAPS NAMED, none hardcoded** (ADR-014 follow-on print-metadata vocabulary): (1) **no document date exists anywhere** — engine fields are id/owner/status/docstatus only, so the header prints `Printed <date>` EXPLICITLY LABELLED and claims nothing more, because an unlabelled date on an invoice reads as the ISSUE date; (2) no doc-level display title, so the header shows the record id; (3) per-field currency scale/symbol. Zero new dependencies. Regression: workflow 18/18, SSE 8/8, Desk unit tests 2/2. See [[2026-07-31 WO-046 document view]].

Prior status — ACTIVE (2026-07-31) — ADR-014's build half
created: 2026-07-31
---

# WO-046: Document View

## Why

ADR-014's decision made concrete. WO-045 found the missing print artifact is not an engine but a **document**: the record page is an editing form (`customer *`, `invoice line lines`, `Total: 15 (computed on save)`), and print CSS cannot make a form into an invoice. The read-only document view is needed *identically* by browser-print today and any future PDF engine — which will consume this same HTML (the one-dialect payoff ADR-014 banked).

## Exit criteria

1. **A read-only document view for any record**, rendered from DocType metadata + record JSON — the same generic-renderer discipline as the form (ADR-004 contract, no per-doctype code): document header (doctype label, record name, workflow state, date), fields as label + value — **no required asterisks, no fieldset legends, no "(computed on save)"** — child lines as a clean table (no remove column, no add-row).
2. **Reached from the record page** (a Print / View-document affordance); the WO-045 `@media print` CSS applies; Ctrl+P yields the invoice-quality one-page PDF.
3. **Money displayed padded-to-scale per the money-formatting ruling** — the display helper the ruling already permitted (presentation only; the stored value is never touched; scale from the field's currency metadata, default 2). `37.5` prints as `37.50`. This is the first customer-facing surface that needs it — build it here, and land the ruling's note in ADR-007 as it specified.
4. **Content-assert the printed output the way the spike did:** the spike's three form-artifacts (asterisk / legend / computed-on-save) asserted **absent**; line rows, amounts, and total asserted **present**. Re-use the spike's harness shape.
5. **One door, unchanged:** the document view reads through the broker/REST under the caller's session — row/field permissions apply identically (a clerk printing sees exactly what a clerk sees). Assert it (clerk vs manager on the same record class).
6. **Zero new dependencies**; behavior regression green (WO-031 18/18, WO-032 SSE 8/8); browser-proven on the real Desk (tested-seam≠wired).

## Boundaries

- **Generic metadata-driven view only.** Custom / app-authored print templates (Frappe's Print Formats) are deferred **with** the engine — they are the same containment question ADR-014 deferred, and they arrive together.
- No PDF engine, no email-attachment wiring — that half activates when the deferred need is pulled.
- Perf hygiene as standing if any number is taken; no scratch left behind.

## Escalation

- If the generic renderer can't express something an invoice genuinely needs (e.g. a doc-level display title vs record id), name it — that's print-metadata vocabulary for the ADR-014 follow-on, not something to hardcode per-doctype.
