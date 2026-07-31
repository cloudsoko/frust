---
tags: [frust, work-order, print, pdf, spike, milestone-4]
status: COMPLETE (2026-07-31) — spike delivered, **NO engine adopted** (as ordered). **CRITERION 0 REFRAMED THE WO: browser print doesn't lack an engine, it lacks a DOCUMENT.** `@media print` works today at zero deps (nav/buttons/actions 0 visible, `remove` column gone 5→4, input borders 0, bg white, 1-page text-extractable PDF) — but it prints the Desk's EDITING FORM: the customer's label is `customer *`, the legend is `invoice line lines`, and the page says `Total: 15 (computed on save)`. The missing artifact is a read-only **document view**, needed IDENTICALLY by every candidate → engine-independent, and the larger half of the job. Measured: **Chrome pooled-CDP 146 ms / 168 MB / 4 procs / 480 MB install** (8/8 content-asserted) vs **CLI spawn 523 ms cold**; **typst 0.15.1 182 ms / 21.5 MB / 52.5 MB single binary** (7/7) but **not HTML — a second dialect** AND its markup is a **programming language**, so app-authored templates would execute in-process (an ADR-005-shaped problem ADR-005 already solved once for plugins). Kernel is 240 crates; typst-as-lib is +267, `fullbleed` +179, `printpdf` +108. **RTL/Arabic done early as ordered: BOTH engines render Arabic correctly (verified on the raster, not a text dump — joining, RTL column order, LTR number islands); they differ in the TEXT LAYER — Chrome logical-order (ligature `الله`→`هللا` on extraction), typst REVERSED/visual-order.** Rejections with verified reasons: **`fullbleed` has NO HTML PARSER** (110 public items, layout primitives + PDF composition only; 376 downloads, created 2026-02-11, `0.6.11→1.6.1` — a 1.x that is a marketing number, WO-006 skepticism saved a 179-crate build); weasyprint = Python/stack-collapse; **wkhtmltopdf verified dead** (`archived:true`, last push 2022-11-22, 1352 open issues). **RECOMMENDATION → ADR: document view + print CSS NOW (zero deps, covers the interactive case fully); engine DEFERRED until WO-043's PDF attachment makes it concrete; then Chrome-CDP by default because it consumes the SAME HTML the document view and browser print already use (one dialect — ADR-007's two-dialects argument), typst only if deployment weight turns decisive (52 vs 480 MB, 21.5 vs 168 MB RSS), price named.** Containment: ADR-010 Tier-2 worker either way, never the request path. **4 instrument failures, all mine and all caught:** Chrome CLI attached to my running browser and reported 828–885 ms with **0-byte output**; `pdftotext` reported ZERO Arabic in a PDF whose Arabic is fine (PyMuPDF gets 97 codepoints — I nearly recorded "Chrome's Arabic is broken"); Chrome memory by process-NAME summed my interactive browser as **5.3 GB / 29 procs**; a `thead` selector on a table with no `<thead>`. Boundary: no dependency in any shipped manifest, all probes in `D:\Dev\rust\wo045-spike\`; the ONE shipped change is the `@media print` block in `frust-desk/src/frust_ui.css` (CSS only, criterion 0's deliverable, WO-031 18/18 still green, revert = delete one block). See [[2026-07-31 WO-045 print pdf spike]].

Prior status — ACTIVE (2026-07-31) — building-block SPIKE that gates an ADR. NO engine is adopted by this WO.
created: 2026-07-31
---

# WO-045: Print/PDF Spike

## Why

The second battery. Grounding (WO-043): the stack has **no HTML→PDF engine anywhere** — the 1288 "print" hits are grammar/pretty-printers/`println`. WO-043's PDF-attachment deferral waits on this. A turnkey ERP must print an invoice. Per convention: new tech = spike with exit criteria → ADR — a tech adoption is never smuggled into a wire-in WO.

## Criterion 0 — the free rung first

Before touching any engine: **`@media print` CSS on the Desk record page.** Browser print covers the interactive case (a user prints an invoice from the Desk) at **zero dependencies** — and the Desk's pages are already single-document, zero-subresource HTML, the ideal print input. Measure what criterion 0 does **and does not** cover. The remainder — server-side PDF (email attachments, archival, API delivery, batch) — is the *only* thing an engine may be adopted for. If the remainder is thin enough, a legitimate ADR outcome is "print CSS now, engine deferred until a concrete need."

## Candidates (measured, not argued)

| candidate | expected shape |
|---|---|
| headless Chrome (CDP) | best HTML/CSS fidelity; heavyweight external binary + lifecycle/containment story to design |
| typst | pure-Rust, excellent output; **not HTML** — a second template dialect (an ADR-007-shaped "two dialects" question, name it honestly) |
| pure-Rust HTML→PDF crates | survey maturity honestly — young ecosystems get the WO-006 skepticism |
| weasyprint | Python — violates stack-collapse; **named to reject with the reason**, not silently omitted |
| wkhtmltopdf | dead upstream; named to reject |

## Exit criteria

1. **Criterion 0 measured** — what browser-print covers, what remains for an engine. Stated as the ADR's framing.
2. **One real invoice rendered** by the leading candidate(s): DocType metadata + record JSON + line table + decimal money — **content-asserted** (amounts, line rows present in the PDF text), never "a PDF file exists."
3. **Numbers:** render latency, dependency/binary weight, memory. ≥3 samples where variance matters.
4. **RTL + Arabic shaping test** — engines differ wildly here and discovering it post-adoption is a rewrite. Cheap to test now, expensive to learn later. A candidate that can't shape Arabic gets that fact in its row, not a footnote.
5. **Containment posture stated** — where rendering runs (kernel thread? worker? external process?): print templates are app-authored content, so the ADR must say who renders them and with what blast radius (the ADR-010 worker posture is the likely shape).
6. **Deliverable: position paper → ADR** — which path, why, rejected alternatives with reasons, per house style. The build WO (print-format DocType, kernel wiring, the WO-043 attachment hookup) comes after the ADR, not inside the spike.

## Boundaries

- Spike only: no kernel integration, no print-format DocType design, **no new deps land in any shipped manifest** — probes live in scratch dirs and die there.
- Full perf hygiene where numbers are taken (quiet machine, scratch store if the kernel is involved).
- **Escalation:** if no candidate clears fidelity + RTL + acceptable weight, that is a *finding* — report it, don't force-pick the least-bad engine.
