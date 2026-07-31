---
tags: [frust, build-log, desk, tier2, bridge, work-order]
created: 2026-07-25
work-order: "[[WO-014 Desk v2 Dynamic Forms]]"
---

# Build Log — WO-014: Desk v2, Dynamic Forms

The six-verb bridge is product. A DocType's **client behaviour is now metadata** — declared, stored, served, and compiled into per-field signals at render time. A `travel_claim` DocType with four behavioural rules was created through the running kernel and its form rendered dynamic **with no recompile and no restart**: ADR-001's Tier-1 property, extended from fields to behaviour.

## Exit criteria

| # | Criterion | Result |
|---|---|---|
| 1 | Metadata vocabulary → per-field signals | ✅ `depends_on`, `read_only_when`, `required_when`, `invalid_when`, `fetch_from` on `FieldDef`; the Desk creates one `Signal<String>` per metadata field in a loop and compiles each rule into an expression over the source field's signal |
| 2 | Zero round-trips, proven | ✅ **network log after exercising every rule: 2 requests — the page and `runtime.js`.** Visibility, read-only, required-marker and money-validation interactions produced **zero** traffic |
| 3 | The lattice still governs | ✅ at Submitted, `visa_ref` (visible only because `depends_on` fired) is frozen exactly like every static field; only `allow_on_submit: notes` stays editable. Dynamism composes with the floor, never outranks it |
| 4 | Decimal discipline in client rules | ✅ boundary-exact: `5000.00` and `5000.000` do not warn, `5000.01` does; `""` and `"abc"` degrade to zero rather than exploding. Comparisons only — see below |
| 5 | Realtime + dynamics coexist | ✅ reconciliation rule stated and proven: a tick against a dirty form does **not** reload; it raises "changed elsewhere" and the in-progress edit survives |
| 6 | Dynamic-signals PR submitted | ✅ [tokio-rs/topcoat#203](https://github.com/tokio-rs/topcoat/pull/203) |

## How a rule becomes behaviour

Runtime expressions are macro-expanded at **compile** time, but a rule's operator is **data**. The renderer resolves this with a `match` per rule: each arm carries its own compiled expression, and the arm is chosen at render time from metadata. That is the whole trick behind "no recompile for a new rule on a new DocType" — a small closed set of compiled shapes, selected by data.

Comparison targets are parsed **once, server-side**, and captured (`Decimal::parse_or_zero(&t)`), so the browser only ever *compares*.

## Criterion 4 in detail — the guarantee held where users hit it

A money rule needs a text input to become a `Decimal`, and the expression vocabulary had no string→decimal step. Rather than route money rules to the server (permitted, but it would have made the common case round-trip), the vendored runtime gained **`Str::to_decimal_or_zero()`** — both language sides, exact, invalid input → zero.

The critical property is what it *cannot* do: `Decimal` still has **no arithmetic operators**, so a client rule can compare money and can never compute a stored value. The WO's "impossible or server-routed" is satisfied by the *impossible* branch — enforced by the type system, not by review.

## The reconciliation rule (criterion 5), stated

> **A live tick may never discard un-saved input.** While a form is dirty, the realtime reload is suppressed and the tick becomes a visible "changed elsewhere" banner. The user resolves it by saving (their write wins, and the lattice + hooks still judge it) or by reloading deliberately (they discard their own edits, knowingly). A clean form still refreshes instantly, so the common case keeps its liveness.

Implemented as a delegated dirty flag; the live script calls `__frustOnTick()` instead of reloading blindly. Staleness is visible, typing is never stomped.

## Vendor additions (both small, both principled)

- **`Js` response wrapper** mirroring `Css` — a route can now serve a module script with a JavaScript media type. This is what lets the Desk keep its single-binary, no-asset-bundle posture *and* have real signals: `runtime.js` is `include_str!`-embedded and served with the right MIME. (Flagged as "the tidier long-term option" in the WO-012 log; now done.)
- **`Str::to_decimal_or_zero()`** as above.

Pushed to vendor `main` (`238e1f4`). **Carried-patch ledger shrank twice this WO:** our #192 fix landed upstream (`c8fb66f`), and the dynamic-signals patch is now a live PR (#203) instead of an indefinite local divergence.

## Findings

1. **`&` is not in the expression language** — `.eq(&t)` is "unsupported expression"; binary operators (`==`, `!=`, `>`, `<`, `>=`) auto-ref and are the supported path. Same family as WO-012's `push_str` finding. Rules therefore compile to binary comparisons throughout.
2. **Reactive effects flush asynchronously** — a test that sets an input and reads the DOM in the same tick reads the *previous* state. Cost one false negative on the money rule before the `await` was added; any future DOM assertion after a signal write needs a tick.
3. **Load order bites inline scripts** — the dirty guard rendered above its form and ran before it existed. Fixed with event delegation from `document` rather than a load event, which is also robust to future markup moves.
4. **Raw-string delimiter collision**: JS containing `"#doc-form"` terminates an `r#"…"#` literal. `r##"…"##` for any script holding CSS/DOM id selectors.
5. **Latency gates cannot share a run with the full suite.** The gates serialize among *themselves* (WO-012) but still measure the other 20-odd binaries' contention: `cargo test` showed the floor failing while an isolated run read **21 ms**. Standing practice extended: **perf gates are a separate invocation**, like the substrate probe — correctness in the suite, latency alone.

## Suite state
13 correctness binaries green; perf gates green **in isolation** (hook 0 ms; floor **21 ms**/25; realtime tax **0 ms**/2). Scratch databases dropped at close (90) per the measuring-WO hygiene set.

## Related
[[WO-014 Desk v2 Dynamic Forms]] · [[ADR-001 UI Extension Tiers]] · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-25 Tier-2 six-verb form bridge]] · [[2026-07-25 WO-012 Desk realtime]] · [[2026-07-25 Decimal surrogate - float-free money in the runtime]] · [[Topcoat]]
