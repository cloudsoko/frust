---
tags: [frust, build-log, print, desk, milestone-4]
created: 2026-07-31
work-order: "[[WO-046 Document View]]"
status: COMPLETE — all 6 criteria met, browser-proven 24/24 on the real Desk. `/print/{doctype}/{key}`: a GENERIC metadata-driven read-only document (no per-doctype code), reached from a Print affordance, WO-045's print CSS applying, Ctrl+P yielding an invoice-quality page. The spike's three form-artifacts (`customer *` / `invoice line lines` / `(computed on save)`) asserted **absent**; content and line rows asserted **present**. Money-formatting ruling built and landed in ADR-007: stored `"15"` displays `15.00`, over-scale values SURFACE rather than round, and the store is asserted unchanged. One door: a clerk prints exactly what a clerk can read, and a record they cannot read is not printable — both asserted, with a vacuity guard. **Caught a live defect in my own WO-045 CSS: an unscoped `table td:last-child` would have eaten the AMOUNT column of this very page.** Zero new deps; workflow 18/18, SSE 8/8 green.
---

# WO-046 — The document view

ADR-014's build half. WO-045 found the missing print artifact is not an engine
but a **document**; this is that document.

## What shipped

| piece | where |
|---|---|
| `GET /print/{doctype_name}/{record_key}` | `frust-desk/src/main.rs` — `print_page` |
| Print affordance on the record page | `doc_page`'s `.fui-page-actions` |
| `pad_money` + `MONEY_SCALE` + `today_iso` | `main.rs` (pure fns, unit-pinned) |
| `.fui-doc*` styles | `frust_ui.css` |
| `print.spec.mjs` (24 checks), `pnpm print` | `frust-e2e/` |
| Money ruling note | [[ADR-007 Tier-2 Script Architecture]] |

**Zero new dependencies** — no manifest changed in kernel, Desk or e2e beyond a
`"print"` script entry.

## Criterion 1 — a document, generically

Rendered from **DocType metadata + record JSON only**. There is no per-doctype
code in `print_page` and none may be added: the scalar fields come from
`dt.fields`, the line columns from the child DocType's own metadata, exactly as
the form does (ADR-004). A DocType created at runtime gets a printable document
with no recompile — the same claim the list and the form already make.

The printed output, from the real seeded invoice:

```
sales invoice                                        Draft
                                     Printed 2026-07-30
z3eeylk9lrjtr1z5s1fh
customer        Beta LLC
total           15.00
workflow state  Draft
ITEM                      QTY  RATE  AMOUNT
Gadget                    3    5.00  15.00
```

Compare the record page's print output that WO-045 measured — `customer *`,
`invoice line lines`, `Total: 15 (computed on save)`. Those are gone, and gone
**by assertion**, not by inspection (criterion 4 below).

## Criterion 3 — money padded to scale, and the store untouched

The ruling's first customer-facing surface. `pad_money` appends zeros to a
string; it never parses to a float, never recomputes a total, never writes.

- stored `"15"` → displays **`15.00`**
- stored `"5.00"` → displays `5.00` (already at scale, untouched)
- **over-scale surfaces**: `1.005` in a 2-place field displays `1.005`, never
  `1.01`. The ruling is explicit that money is stored *at* scale, so extra
  places are a defect to show a human, not to tidy away.
- non-decimals (`n/a`, `$5`, `1e3`, empty) pass through rather than being
  half-formatted.

Unit-pinned in `main.rs`, including a property check that padding never alters
the numeric value. And asserted in-browser that the **stored value is unchanged
after rendering** — because "presentation only" is a claim about writes, so the
test checks writes.

**Scale is 2 everywhere, and that is named not hidden:** DocType metadata has no
`precision` field, so there is nothing per-field to read. A per-field scale and a
currency symbol are **print-metadata vocabulary for the ADR-014 follow-on**
(criterion-escalation shape), not something to hardcode per doctype.

## Criterion 4 — asserted the spike's way

`pnpm print`, 24 checks, real Chromium against the live Desk. **Absent:**
required asterisk · `(computed on save)` · fieldset legend · any `<input>` ·
any `<form>` · `+ Add row` · a `remove` column. **Present:** customer · record
name · line item · workflow state · every line column · the amounts.

## Criterion 5 — one door

The view reads through the kernel under the **caller's own session** — the
identical `/read/{doctype}` call the record page makes. Row and field
permissions therefore apply by construction, not by a second check that could
drift.

Asserted both directions: manager sees 33 rows, clerk1 sees 29; a clerk **can**
print a record they can read, and a manager-only record **is not printable** by
the clerk (`sales_invoice:18gri2ra8b1jlzhi929h` → "isn't yours to see"). The
refusal case carries a **vacuity guard**: if no manager-only record existed the
check fails loudly rather than passing on an empty set — the WO-038/WO-043
lesson applied again.

## Finding — my own WO-045 CSS would have eaten the amount column

WO-045's print block hid the line editor's `remove` column with:

```css
table th:last-child, table td:last-child { display: none !important; }
```

Unscoped. On the **document view** the last column is `AMOUNT`. That stylesheet
shipped one WO ago and was green, because the only table it had ever met was the
editor's. Scoped to `form table …` and pinned by an assertion that every line
column survives print (`[4,4]` cells vs 4 headers) — a check that fails if the
rule ever widens again.

This is the WO-042 finding recurring exactly as predicted: **the Rust↔CSS seam
has no type system, and a selector that is merely too broad fails silently.**
WO-042 also suggested the guard for it — grep referenced-vs-defined custom
properties — so I ran that too: **71 defined, 66 referenced, 0 undefined**. It
caught one of mine (`--fui-outline`, which does not exist; the real token is
`--fui-outline-1`) before it shipped.

## Named gaps (escalation shape, not hardcoded)

1. **No document date exists.** A record carries `id`, `owner`, `status`,
   `docstatus` and its fields — there is no issue/created date anywhere in the
   engine fields. An invoice genuinely needs one. The header therefore prints
   **`Printed <date>`, explicitly labelled**, and claims nothing more: an
   unlabelled date on an invoice reads as the *issue* date, and printing today's
   date where a customer expects the issue date is a real business error. A
   document date is ADR-014-follow-on vocabulary.
   (The date is UTC — consistent with how the kernel stores `time::now()` — so
   it can read one day behind a late-evening local clock. Deliberate; labelled.)
2. **No doc-level display title.** The header shows the DocType label and the
   record *id* (`z3eeylk9lrjtr1z5s1fh`), because nothing designates a
   human-facing title field. The WO named this exact example; it is the same
   follow-on vocabulary.
3. **Per-field currency scale / symbol** — as above.

None of these were worked around per-doctype.

## Regression

`pnpm workflow` 18/18 · `pnpm sse` 8/8 · `pnpm print` 24/24 · Desk unit tests
2/2. The `@media print` scoping fix is the only change to existing behaviour and
it is a *narrowing*, verified by the new column assertion.

## Related
[[WO-046 Document View]] · [[ADR-014 Print Strategy]] ·
[[2026-07-31 WO-045 print pdf spike]] (found the missing artifact) ·
[[ADR-007 Tier-2 Script Architecture]] (the money ruling's home) ·
[[ADR-004 Topcoat for Desk v0]] · [[2026-07-29 WO-042 frust ui re-skin]]
