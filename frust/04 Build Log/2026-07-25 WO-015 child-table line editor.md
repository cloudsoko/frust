---
tags: [frust, build-log, desk, child-tables, work-order]
created: 2026-07-25
work-order: "[[WO-015 Child-Table Line Editor]]"
---

# Build Log — WO-015: Child-Table Line Editor

The canonical ERP form is complete: an invoice with lines, created at runtime, edited in the browser, landing in the aggregates ladder. **The loop opened in WO-007 is closed — a line edited in the Desk moved the item-wise Tier-2 rollup.**

## Exit criteria

| # | Criterion | Result |
|---|---|---|
| 1 | Metadata-driven line rendering | ✅ `inv_line` child DocType created at runtime; its own field metadata produced the Item/Qty/Amount columns with **no recompile** |
| 2 | Row ops through the envelope; rollup sees them | ✅ whole embedded array submitted on save; **`widget qty 3 → 5` and `amount +150.0` propagated into `sales_by_item2`** via WO-007's line-differ |
| 3 | Line-level dynamics compose | ✅ row-scoped signals (money signal per rendered row); reveal-by-signal add-row runs client-side with zero round-trips |
| 4 | Decimal per line; total server-owned | ✅ decimals in/out per row; **`total: 100.00 in → 115.0 stored`** — the hook chain owns it, the UI shows the stored value labelled *"(computed on save)"* and never fakes one |
| 5 | The lattice on lines | ✅ at Submitted: **0 editable inputs in the table, no Add-row button**, rows render as frozen text with the rest of the form |
| 6 | Line history in the audit UI | ✅ `~ widget · qty: 3 -> 5` — per-field line diff computed **presentation-side** from whole-document feed entries. ADR-008 A4's promissory note is cashed |

## Design: adding a row without creating DOM

Existing rows plus `SPARE_ROWS` blanks render up front, each with its signals; **"+ Add row" reveals the next blank by incrementing one counter signal** (`:hidden=$(!(shown.get() > idx))`). So a new row costs no DOM creation, no round-trip, and — critically — arrives already reactive, which a cloned row would not.

That choice is also the scope tripwire honoured: this is row reveal + per-row fields, **not** per-cell reactivity, bulk edit, or ranges. ADR-004 revisit-trigger #1 stays armed and was not approached.

Removal is a per-row checkbox; on save, removed and never-filled rows collapse to the same thing (dropped). The whole array goes through `db_write` every time — which is precisely what lets hooks and the Tier-2 differ see a document rather than a patch.

## Two bugs found, both silent-wrong (the house-style enemy)

1. **The typed envelope did not decode inside nested arrays.** `parse_value` handled `{kind,v}` at the top level, but arrays recursed through `infer_value`, which does not. Line amounts therefore landed in the database as *literal envelope objects* (`{"kind":"decimal","v":"150.00"}`) instead of decimals. Fixed in `rest.rs`: `infer_value` now recurses through `parse_value`, so a typed envelope decodes **at any depth**. This bug was invisible until a child table existed — embedded rows are the first arrays-of-objects the envelope ever carried.
2. **The Tier-2 line-differ silently rolled up ZERO for money.** `ItemSales::contrib` used `as_f64()`, which returns `None` for a decimal (SurrealDB serialises decimals as JSON strings). Written in WO-007 when test amounts were floats; WO-014's decimal discipline made real amounts strings, so every line amount aggregated as 0 — a wrong number with no complaint. Fixed with an explicit `numeric()` that accepts number **or** decimal-string, verified by the `+150.0` delta above.

## Finding for the PM — REQ-6.2.1 gap in the aggregates layer

Fixing bug 2 exposed a deeper one, flagged rather than silently redesigned: **Tier-2 rollups accumulate money as `f64`.** The rollup table's `amount` is a float and the delta accumulator is float arithmetic — predating the Decimal surrogate entirely (WO-007 shipped before WO-014). Parsing to `f64` is strictly better than silently zeroing, but it is still money-as-float inside the aggregates layer, which REQ-6.2.1 forbids at boundaries. The real fix is decimal-typed rollup accumulation; that is an ADR-010 amendment and a WO of its own, not a line-editor改. Recorded here so it is in daylight.

## Notes

- The known WO-007 seeding trap fired again (declaring an aggregate reshapes an already-synced rollup into write-closed form → destructive gate refuses). Its documented resolution — drop the rollup table + its migration row, re-sync — worked unchanged. The caveat earned its ink twice now; worth promoting to the metadata docs as "declare aggregates before first sync of their rollups".
- No `lines.*` FLEXIBLE surprise: the ADR-008 embedded-child DDL handled arrays-of-objects as documented (checked the caveat first, per the WO).

## Suite state
**25 correctness binaries green, zero failures.** Perf gates green in isolation (hook 0 ms; floor **23 ms**/25; realtime tax **0 ms**/2). Substrate probe 20.8 ms before the run; 50 scratch databases dropped at close.

## Related
[[WO-015 Child-Table Line Editor]] · [[ADR-008 Data Shape]] (A4) · [[ADR-004 Topcoat for Desk v0]] (trigger #1, untouched) · [[ADR-010 Materialized Aggregates]] · [[2026-07-25 WO-014 Desk v2 dynamic forms]] · [[2026-07-24 WO-007 aggregates ladder implementation]]
