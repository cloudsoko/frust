---
tags: [frust, build-log, spike, print, pdf, position-paper, milestone-4]
created: 2026-07-31
work-order: "[[WO-045 Print PDF Spike]]"
status: COMPLETE — spike done, NO engine adopted (as ordered). **Criterion 0 changed the question.** Browser print works today at zero dependencies and produces a legible, text-extractable, correctly-paginated PDF — but it prints the Desk's EDITING FORM, not a document. The missing artifact is a read-only DOCUMENT VIEW, and every candidate needs it equally, so it is engine-independent and is the larger half of the job. Recommendation: **document view + print CSS now (zero deps, covers the interactive case fully); engine DEFERRED until WO-043's PDF attachment makes it concrete; Chrome-CDP the default when it arrives (one template dialect), typst the alternative if weight dominates.** All candidates measured: Chrome-CDP 146 ms/168 MB/480 MB binary; typst 182 ms/21.5 MB/52 MB binary but a SECOND DIALECT and a scripting language in-process; `fullbleed` has NO HTML PARSER (description overclaims); wkhtmltopdf archived 2022-11-22 (verified). Both engines render Arabic correctly; they differ in the TEXT LAYER.
---

# WO-045 — Print/PDF spike: position paper

No engine is adopted here. This gates an ADR.

## Criterion 0 first, and it reframed the WO

**`@media print` on the Desk record page, zero dependencies, works.** The
stylesheet landed in `frust-desk/src/frust_ui.css` (CSS only — no dependency, no
kernel change, no DocType). Browser-proven on the real seeded invoice:

| | screen | print |
|---|---|---|
| nav / buttons / page + form actions | visible | **0 visible** |
| line-table columns | `item qty rate amount` **`remove`** | `item qty rate amount` |
| input borders | drawn | **0** (values read as text) |
| body background | theme-dependent | `rgb(255,255,255)` |

The resulting PDF is one page, 83 KB, and its text extracts cleanly:

```
sales invoice z3eeylk9lrjtr1z5s1fh   Draft
customer *          Beta LLC
item      qty   rate    amount
Gadget    3     5.00    15.00
Total: 15 (computed on save)
```

**And that output is the finding.** Read it as an invoice: the customer's label
is `customer *` — a required-field asterisk. `invoice line lines` is a fieldset
legend, i.e. a field name. `Total: 15 (computed on save)` is a UI hint that has
no business on a document a customer receives. There is no issue date, no
letterhead, no totals block, no terms.

> **Browser print doesn't lack an engine. It lacks a document.**
> The Desk's record page is an *editing form*, and print CSS can make a form
> tidy but cannot make it an invoice.

That reframes the whole WO, because **the missing artifact — a read-only
document view — is needed identically by every candidate.** Browser print needs
it. Chrome needs it. typst needs it (in its own dialect). It is the larger half
of the work and it is engine-independent, so it can be built *now* and it
commits us to nothing.

**What criterion 0 covers, once a document view exists:** the entire interactive
case — a user opening an invoice and pressing Ctrl+P, with the browser's own
print preview, page-size choice, and print-to-PDF. At zero dependencies.

**What remains for an engine:** PDF produced with **no browser present** —
email attachments (WO-043 deferred exactly this), archival, API delivery, batch
runs. Real, but every one of them is currently *deferred*, not *demanded*.

## The candidates, measured

Same real invoice (`Beta LLC`, `Gadget` 3 × `5.00` = `15.00`), rendered by each.
Latency ≥3 samples; the invoice's content asserted **inside** the PDF, never
"a file exists".

| candidate | render latency | binary / dep weight | memory | HTML in? | content assert |
|---|---|---|---|---|---|
| **browser print** (criterion 0) | user's browser | **zero** | user's | the Desk page itself | 8/8 |
| **Chrome, pooled CDP** | **146 ms** median (134–160), +96 ms one-time launch | 480 MB Chrome install; `headless_chrome` +91 crates as a client | **168 MB / 4 processes** | **yes** | 8/8 |
| Chrome, CLI spawn per render | 523 ms cold profile / 683 ms warm | same | same | yes | same |
| **typst 0.15.1** | **182 ms** median cold spawn (174–314) | **52.5 MB single binary**; +267 crates as a library | **21.5 MB / 1 process** | **no — own markup** | 7/7 |
| `fullbleed` 1.6.2 | — | +179 crates | — | **NO** | — |
| `printpdf` | — | +108 crates | — | no (primitives) | — |
| weasyprint | — | Python runtime | — | yes | — |
| wkhtmltopdf | — | — | — | yes | — |

For scale: the kernel is **240 crates** today. typst-as-a-library would more
than double it.

### Rejections, with reasons

- **`fullbleed`** advertises "Deterministic HTML/CSS-to-PDF engine in Rust".
  **It has no HTML parser.** Its 110 public items are programmatic layout
  primitives (`Document`, `Page`, `Paragraph`, `Frame`, `Flowable`) plus
  PDF-composition helpers; the only html-named items are `HtmlTableFacts` and
  `PageHeaderHtmlSpec`, and there is no `html` feature. Also: created
  2026-02-11, **376 total downloads**, versions jumping `0.6.11 → 1.6.1`. A 1.x
  on a five-month-old crate is a marketing number, not a maturity signal — the
  WO-006 skepticism earned its keep and saved a 179-crate build.
- **`printpdf`** is a PDF *writer*, not an HTML engine. Correctly scoped, wrong
  layer for us.
- **weasyprint** — Python. Rejected on stack collapse: a Rust ERP that shells to
  a Python runtime to print an invoice has reintroduced the deployment surface
  the rewrite exists to remove.
- **wkhtmltopdf** — **verified dead**, not assumed: GitHub reports
  `archived: true`, last push **2022-11-22**, 1352 open issues. Built on
  long-obsolete QtWebKit.

### RTL + Arabic (criterion 4, done early on purpose)

Both engines **render Arabic correctly** — verified by looking at the raster, not
by trusting a text dump: right-aligned, RTL table column order, correct cursive
joining, LTR number islands (`15.00`) correct inside RTL runs. typst even
composes the `الله` ligature properly.

They differ where it is less visible — **the text layer**, which is what makes a
PDF searchable, copyable and machine-readable:

| | Chrome | typst |
|---|---|---|
| pure-Arabic runs | **logical order, correct** | **reversed (visual order)** |
| `الله` ligature | decomposes to `هللا` on extraction | renders right; extraction reversed |
| bidi-mixed line | reordered on extraction | reordered on extraction |
| base letters vs presentation forms | base letters (0 presentation forms) | — |

**Chrome wins the text layer; neither is perfect.** For an ERP this matters:
"can you search your Arabic invoices" and "can an auditor extract the text" are
real questions. Neither engine disqualifies itself, and — importantly — **this
was cheap to learn now** exactly as the WO predicted.

### Containment (criterion 5)

Print templates are **app-authored content**, so who renders them is a security
question, not a deployment detail.

- **Chrome** renders in an **external process**. Strong isolation (it is a
  browser sandbox), heavy, and the lifecycle is real work: a pool to keep, a
  crash to survive, a zombie to reap. HTML+CSS for print is comparatively inert
  and JS can be disabled outright.
- **typst** would render **in-process as a library** — and **typst's markup is a
  programming language** (`#let`, `#for`, function calls; even the invoice
  template above is function calls). App-authored typst executing inside the
  kernel's address space is an ADR-005-shaped problem that ADR-005 solved for
  plugins with wasmtime, and would have to be solved again here. That is a
  bigger cost than its 21.5 MB suggests.

Either way the **ADR-010 Tier-2 posture** is the shape: rendering happens on a
background worker, never on the request path — the same rule WO-043's mail
worker follows, for the same reason (a slow render must slow the document, never
the save).

## Recommendation for the ADR

1. **Build the read-only document view.** It is required by every option, it is
   engine-independent, and it is the larger half of the work. Today it does not
   exist and *that* — not the absence of an engine — is why Frust cannot print an
   invoice.
2. **Ship print CSS with it.** The interactive case is then fully covered at
   zero dependencies. (The stylesheet already exists; see below.)
3. **Defer the engine** until the concrete need lands — WO-043's PDF attachment
   is the natural trigger. "Print CSS now, engine deferred" is the WO's own
   sanctioned outcome and the evidence supports it: the remainder is real but
   entirely deferred work.
4. **When the engine is needed, Chrome-CDP is the default**, for one reason that
   outranks its weight: **it consumes the same HTML the document view and browser
   print already use.** One template dialect, authored once. typst would mean
   authoring every document twice, or abandoning browser print — and ADR-007's
   two-dialects argument is precisely this shape. typst becomes the answer if
   deployment weight turns decisive (52 MB vs 480 MB, 21.5 MB vs 168 MB RSS is a
   genuinely large gap), and the price is named: a second dialect and an
   in-process scripting language to contain.
5. **If neither is acceptable, the honest answer is browser-print-only** — and
   that is a legitimate v1 for an ERP whose users print from a browser.

The build WO (document view, print-format DocType, the WO-043 attachment
hookup) comes after the ADR.

## Instrument failures, mine

1. **Chrome CLI attached to my already-running browser** and reported 828–885 ms
   of render latency with **0-byte output** — a plausible number for a render
   that never happened. `--user-data-dir` isolates it. The tell was the file
   size, not the timing.
2. **`pdftotext` reported ZERO Arabic** in a PDF whose Arabic is fine. I nearly
   recorded "Chrome's Arabic text layer is broken". PyMuPDF extracts 97 Arabic
   codepoints from the same file and the fonts carry `ToUnicode` maps — the
   extractor was the broken instrument. Cross-checked across three libraries
   before attributing anything.
3. **Chrome memory measured by process NAME** summed my interactive browser:
   **5.3 GB across 29 processes**, reported as if it were the renderer. Filtering
   to the Playwright binary gives 168 MB across 4. I flagged the doubt in the
   probe's own output before believing it.
4. A probe read `table thead th` on a table with no `<thead>` and reported zero
   visible columns — which looked like the print CSS hiding the whole table. The
   PDF text (4 columns, `remove` gone) disproved it.

## Boundary compliance

- **No engine adopted. No dependency added to any shipped manifest** — verified:
  `frust-kernel` and `frust-desk` manifests untouched; all probe crates,
  binaries and PDFs live in `D:\Dev\rust\wo045-spike\` and die with it.
- The **one** shipped-tree change is `frust-desk/src/frust_ui.css`: a
  `@media print` block, CSS only, no dependency — it *is* criterion 0's
  deliverable. Screen rendering is untouched (it is inside `@media print`) and
  **WO-031 workflow 18/18 still passes**. Revert is one block deletion if the
  ADR prefers to hold it.
- typst was evaluated from its **released binary**, downloaded into the scratch
  dir — not added to a manifest.

## Related
[[WO-045 Print PDF Spike]] · [[2026-07-30 WO-043 email batteries]] (the deferred
PDF attachment this unblocks) · [[ADR-004 Topcoat for Desk v0]] ·
[[ADR-005 Plugin Isolation]] (the containment shape typst would reopen) ·
[[ADR-007 Tier-2 Script Architecture]] (the two-dialects argument) ·
[[ADR-010 Materialized Aggregates]] (the Tier-2 worker posture)
