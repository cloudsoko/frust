---
tags: [frust, adr, print, pdf, desk]
status: ACCEPTED 2026-07-31 — ratified from WO-045's measured position paper
decided: 2026-07-31
---

# ADR-014: Print Strategy — Document View First, Engine Deferred

## Decision

1. **Interactive printing ships NOW at zero dependencies:** a read-only **document view** + the `@media print` CSS (WO-045 criterion 0, already shipped in `frust_ui.css`). The spike's central finding governs: *browser print didn't lack an engine, it lacked a document* — the record page is an editing form, and print CSS can tidy a form (`customer *`, `invoice line lines`, `Total: 15 (computed on save)`) but cannot make it an invoice. The document view is **engine-independent and the larger half of the job** — every candidate needs it identically.
2. **A server-side PDF engine is DEFERRED** until a concrete need arrives — first candidate: WO-043's email PDF-attachment deferral. Adopting an engine now is tech ahead of need; "print CSS + document view now, engine deferred" is the spike's own sanctioned outcome.
3. **When the need arrives: pooled Chrome-CDP is the default.** The reason that outranks its weight: it consumes the **same HTML** the document view and browser print already use — one dialect, authored once (ADR-007's two-dialects argument in a new place). Measured: 146 ms pooled (523 ms CLI-spawn — the pool is mandatory), 168 MB RSS / 4 procs isolated-by-path, 480 MB install, 8/8 content-asserted, Arabic rendered correctly.
4. **typst 0.15.1 is the named alternative** if deployment weight turns decisive: 182 ms cold spawn (no resident browser), 21.5 MB RSS, 52.5 MB single binary — at two named prices: a **second template dialect** (not HTML), and its markup is a **programming language**, so app-authored templates would execute in-process — the ADR-005 containment problem re-opened. Price stated, not hidden.
5. **Containment when the engine lands:** ADR-010 Tier-2 worker posture — rendering never on the request path, either engine.

## Rejected (verified, not assumed)

- **`fullbleed`** — advertises "HTML/CSS-to-PDF engine in Rust"; its public API (110 items) **has no HTML parser** — layout primitives + PDF composition only. 376 downloads, created 2026-02-11, versions `0.6.11→1.6.1` (a 1.x that is a marketing number). WO-006 skepticism saved a 179-crate build.
- **wkhtmltopdf** — dead upstream: `archived: true`, last push 2022-11-22, 1352 open issues.
- **weasyprint** — Python; stack-collapse (P-2.3).

## Measured caveats

- **Arabic text-layer ≠ Arabic rendering.** Both engines *render* Arabic correctly (verified on the raster: cursive joining, RTL column order, LTR number islands). The **text layer** differs: Chrome keeps logical order but decomposes the lam-lam-heh ligature on extraction (`الله`→`هللا`); typst emits visual order (words reversed). Search/extraction caveat, not a display defect — neither disqualifies.
- **`pdftotext` is an unreliable Arabic extractor** — reported *zero* Arabic in a PDF carrying 97 codepoints (PyMuPDF found them). Verify shaping on the raster and cross-check extractors before attributing a text-layer defect to an engine.

## Evidence

[[2026-07-31 WO-045 print pdf spike]] · [[WO-045 Print PDF Spike]] · build WO: [[WO-046 Document View]]
