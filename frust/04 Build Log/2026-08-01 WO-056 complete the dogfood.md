---
tags: [frust, build-log, dogfood, desk, maturity, survey, milestone-5]
created: 2026-08-01
work-order: "[[WO-056 Complete the Dogfood]]"
status: DELIVERED as a SURVEY — the lifecycle runs end to end in a browser except EMAIL-TO-CUSTOMER, which is not completable. 23 gaps found and categorized; 2 fixed as cheap Desk glue. ONE ESCALATION — a write refused by the database reports HTTP 200 "created" with a null record (WO-020's Finding A, alive on the CREATE path). The alpha held silently throughout; its one apparent intrusion was a Desk dead-end, not containment.
---

# WO-056 — Complete the Dogfood: living in the invoicing app

## What was driven, clicked, in a real browser

Home → create Customer ("Meridian Logistics") → create Item ("Consulting Hour",
150.00) → draft Sales Invoice with a line → **deliberately unbalanced save**
(refused) → corrected → **Submit as clerk** → **Approve as manager** → record a
**Payment** (120.00) → **AR moves** → **Print** → **AR report** → edges.

**The money loop closes without anyone computing anything.** After approval and
payment, `ar_outstanding` for the customer reads `charged 300, paid 120, n 2`,
maintained by the Tier-1 EVENT. Nobody pressed "recalculate."

**Step 7 — email the invoice to the customer — is NOT completable.** See G14/G19.

## THE ESCALATION — a refused write reports success

`POST /write/ar_outstanding` (a write-closed rollup) as manager:

```
HTTP 200   {"action":"created","created":null,"record":null}
```

**and no row is created.** Reproducible at the door and through the browser.

The containment is *correct* — the rollup is write-closed by design (WO-007) and
the database refused the write. What is wrong is the **report**: the kernel
answers 200 with `action: "created"` for a write that did not happen, and the
trace logs `ok:true`.

This is **WO-020's Finding A** — *"permission-refused UPDATE returns Ok([]) not
an error — Frust swallowed it"* — which was closed with a typed
`E_WRITE_NO_ROWS` in `db_write_inner`. That guard evidently covers the UPDATE
path; **a CREATE that writes zero rows is not caught.** Silent-wrong class, on
the write door.

Per the WO's escalation clause this is **reported, not fixed** — it is a kernel
correctness bug, not glue, and it deserves its own order. One note for whoever
takes it: WO-055's additive `action`/`record` keys are what made it *visible*
(`action:"created"` beside `record:null` is self-contradicting), which is a small
argument for that WO's shape.

## The gap list

### missing-glue — Desk-buildable (the bulk, as predicted)

| # | gap |
|---|---|
| G1 | **No home.** Landing page was an alphabetical catalogue of every DocType — `thing`, `WO-042 order`, child tables and rollups beside `sales invoice`. A developer's view of a database. **FIXED** |
| G2 | **Empty state blamed a filter nobody set** — "nothing matches this filter" on a first-run empty table. **FIXED** |
| G3 | **No child-table editor on the NEW form.** `/form/{doctype}` has no `Table` branch at all, so `lines` renders as a bare textbox — you cannot create an invoice with its lines in one pass. Workaround: save an empty invoice, then edit it. |
| G4 | **A validation refusal discards typed lines.** The unbalanced save was correctly refused with a good message; the form then re-rendered from stored state and the user's typed row was gone. |
| G5 | **The Reports page lists one rollup twice** — one entry per aggregate declaration (`from payment`, `from sales_invoice`), two identical links to the same URL. |
| G6 | **The AR report cannot show what anyone owes.** It renders one declaration's metrics, so `paid` shows and **`charged` is absent**; the `count` column is blank for every row though `n` is stored. The stored row has all three. |
| G7 | **Records are identified by random id everywhere** — "sales invoice chn0lz0xx9bbor12cxzq" in headings, lists and print. |
| G8 | **Money renders inconsistently:** `300` on the form and list, `300.00` on print — `pad_money` is applied only on read-only views. |
| G9 | Developer caption on every form: *"Fields react to each other in the browser — no round-trips except `fetch_from`."* |
| G10 | `workflow state` is an editable free-text field on the form, beside the buttons that own it. |
| G11 | Child tables and write-closed rollups offer **"New"** (`invoice line`, `ar outstanding`) — see the escalation for where that leads. |
| G12 | After Submit the clerk sees no workflow panel at all — correct (manager-only actions) but nothing says "waiting for approval". |
| G13 | A list is silently ownership-filtered; two users see different counts under the same title with no indication. |
| G23 | The Reports page links are unstyled raw blue. |

### missing-feature — app-level, buildable as bundle content

| # | gap |
|---|---|
| G14 | **`customer` has no email field** — only `cust_name`. The customer cannot be mailed at the data level. |
| G15 | **The app validates arithmetic it refuses to perform.** `amount` and `total` are hand-typed; the script only *checks* `lines == total` and refuses if not. `Decimal` has been in the sandbox since WO-030 — the seed's script could compute both. |
| G16 | `customer` and `item` on the invoice are `Data`, not `Link` — free-text names, no picker, no referential integrity. The item I created was never actually connected to the invoice line that names it. |
| G17 | No **outstanding** concept (`charged − paid`) and no aging buckets — the numbers exist, the business question isn't asked. |
| G18 | The printed invoice has no seller identity, invoice number, issue date, customer address or terms. It is a faithful record, not a document you could send. |

### missing-in-core — needs a kernel capability, NOT built here

| # | gap |
|---|---|
| G19 | **No user-initiated "send this document."** WO-043's email is event-triggered rules only; there is no surface to enqueue an ad-hoc send of a chosen document to a chosen recipient. "Email this invoice" has nowhere to hook. → own WO |
| G20 | **No human document identity.** The kernel mints random ids; there is no naming-series or display-title concept, which is what makes G7 pervasive rather than cosmetic. → own WO/ADR |
| **ESC** | **A refused CREATE reports 200 "created" with a null record** (above). → own WO |

### clumsy-but-works

G21 save-then-edit to add lines · G22 approval needs a real role switch (correct
by design, but there is no "act as" for a solo operator testing a flow).

## The alpha, tested by silence

**It never once demanded a thought during a legitimate flow.** Recorded because
that is the strongest result containment can have:

- **Row permissions** — the clerk owned what they created and never met a wall.
- **Role filtering** — clerk saw no Approve/Reject; manager saw both. No
  explanation needed, none offered, none wanted.
- **The lattice** — after approval the record simply froze: fields read-only, no
  Save, no remove column. It never announced itself.
- **The sandbox** — the `acct` script ran on every save invisibly; the only time
  it spoke was to refuse the unbalanced invoice, in its own words, naming its
  app: *"Invoice does not balance: lines sum to 300.00 but total is 0.00
  [FRUST:E_INVOICE_UNBALANCED] (rejected by the owner, app 'acct')"*.
- **The dirty guard** — "Unsaved changes — live refresh is paused while you
  type" appeared exactly when it should.

**No containment intrusion was found.** The one candidate — creating a rollup
row dead-ending on a 404 — is the Desk mishandling a correct refusal, not the
moat blocking a legitimate act.

## Built here (cheap Desk glue only, core untouched)

- **A home workspace.** Cards are **derived from metadata** — a DocType is a
  "start here" card if it is submittable — so an app installed tomorrow gets its
  own cards with no recompile. The full DocType table stays below for the
  developer view. *Honest limit:* "submittable" is a starting heuristic, not a
  finished workspace; `customer` and `item` are things you create constantly and
  get no card.
- **An empty state that tells the truth** — "No Invoice yet — create the first
  one" vs "Nothing matches this filter." Both branches verified.
- `.fui-cards` / `.fui-card__actions` added to the Desk CSS, and the
  referenced-vs-defined guard re-run (0 undefined classes) per the WO-042 seam
  lesson.

**Zero kernel source changed** (`git status` confirms), which is the WO-022 rule
holding: every platform gap above is logged, not patched.

## Maturity read — the distance from 55 features to a product

**The engine is further along than the product.** Everything that computes,
enforces or refuses is genuinely good: money is exact end to end, the lattice
and roles are invisible until they matter, refusals carry the rule's own words
and the app that raised them, AR maintains itself. Nothing in the core embarrassed
itself under real use.

**What is missing is almost entirely the layer a person actually touches** — and
it is missing consistently, which is a better sign than it sounds: the gaps are
one kind of work, not scattered defects. A blunt read: **the platform is
mid-beta, the application is early-alpha.**

### Top 5 between here and "a real person could use this"

1. **Documents need human identity** (G7/G20). Every screen names records by
   `chn0lz0xx9bbor12cxzq`. Nothing else on this list is felt as often.
2. **The AR report must show what is owed** (G6/G17). The accounting app's
   headline report currently cannot answer its own question, though the data is
   there.
3. **Create an invoice in one pass, and never lose typed lines to a refusal**
   (G3/G4). Today the first invoice takes a save-then-edit detour, and one
   mistake costs the typing.
4. **Send the invoice** (G14/G19/G18). Email exists but cannot reach a customer,
   and the printed document isn't one you'd send.
5. **Let the app do the arithmetic it already validates** (G15). Being refused
   for a sum the machine could compute is the sharpest "unfinished" signal in
   the flow.

Fix 1–3 and the app becomes usable-with-workarounds. Add 4–5 and a real person
could run a small business on it.

## Regression + dev-store state

Desk unit tests 4/4; WO-031 workflow suite ALL PASSED; WO-032 SSE suite ALL
PASSED (which also covers the survey's "second view self-updating" edge). Kernel
untouched.

Dev store gained: customer "Meridian Logistics", item "Consulting Hour" (150.00),
sales_invoice `chn0lz0xx9bbor12cxzq` (300.00, Approved), payment
`pzugcvjf3w4fiiicqed3` (120.00, submitted), and the resulting AR row. Screenshots
in `frust-e2e/` (`wo056-01`…`wo056-10`).

## Related
[[WO-056 Complete the Dogfood]] · [[2026-07-26 WO-022 accounting seed dogfood]] ·
[[WO-020]] (Finding A, the escalation's ancestor) · [[ADR-010 Aggregate Ladder]] ·
[[ADR-014 Print Strategy]] (the print-metadata vocabulary G18 needs)
