---
tags: [frust, build-log, money, decimal, scripts, boa, wasm, work-order, v1.1]
created: 2026-07-28
work-order: "[[WO-030 Decimal for Scripts]]"
status: complete — P-3.4 bounded→killed; decimal.rs runs in the sandbox verbatim; three hosts one answer
---

# Build Log — WO-030: Decimal for Scripts

The first v1.1 verdict, moved by building. P-3.4 — float-based money, the pain
the whole project is a reaction to — was the *canonical bounded* because one
surface still lacked exact decimal: app scripts. Closed. `decimal.rs` now runs
inside the Boa sandbox, the same arithmetic in three hosts.

## The shape of the fix — and why it's the right one

The script engine is a **wasm component** (`wasm-spike/script-engine`) that
embeds Boa; the kernel loads it, the Desk loads a browser transpile of it. Two
ways to give scripts decimal math:

1. **A host function** — add `decimal-mul` etc. to the WIT world, compute in the
   kernel, call across the guest→host boundary per op.
2. **Compile `decimal.rs` INTO the guest** and register a Boa `Decimal` global.

Chose (2), and it is better on the criterion that matters most — **containment
(criterion 6)**. Option 1 adds a new host capability (a new WIT import is new
authority the sandbox can reach). Option 2 adds **no capability at all**: it is
pure arithmetic compiled into the guest, no I/O, no import, nothing the WO-017
hostile posture has to re-examine. The binding makes the sandbox *more
capable* without making it *less contained* — and those are usually in tension.

It also satisfies the WO's load-bearing boundary — *reuse `decimal.rs` verbatim,
never reimplement decimal in JS* — literally: the guest compiles the kernel's
actual source file. One source, two compile targets, so the three hosts cannot
drift by construction, not by discipline.

### The `include!` snag, and the 6-line build.rs

`decimal.rs` opens with a `//!` module-doc header, and `include!` **forbids
inner doc-comments** (E0753). Rather than mangle the shared source or extract a
crate (heavy: it would restructure the kernel workspace to serve the guest), a
`build.rs` copies the canonical file into `OUT_DIR` each build and demotes only
its leading `//!` lines to `//`. `cargo:rerun-if-changed` on the source means
any edit re-copies — the verbatim guarantee holds, and the transform touches
comments only, never a byte of arithmetic.

*Ladder note:* tried the higher rung (`include!` directly) → didn't hold
(inner-doc limitation) → dropped to the next that does (build-time copy), not
all the way to a shared crate. The crate is the right move if a third consumer
appears or the file grows crate-internal deps; today it would be structure for
its own sake.

## The binding

A `Decimal` global, string-in / string-out (money crosses this boundary as a
string, same as it crosses the WIT and DB boundaries):

```js
Decimal.add(a, b)   Decimal.sub(a, b)
Decimal.mul(a, b)                    // EXACT — scale grows, round explicitly
Decimal.div(a, b, scale, mode)       Decimal.round(a, scale, mode)
Decimal.cmp(a, b) -> -1 | 0 | 1      // numeric, so "1.50" == "1.5"
```

- **Rounding is never implicit (criterion 2, REQ-6.2.2):** `mul` grows the scale
  and returns the exact product; the author must `round` at a defined point.
  `div`/`round` take scale + mode explicitly; mode defaults to half-even (the
  accounting default) but is a per-call argument — there is no global config.
- **Failures are typed:** overflow → `E_MONEY_OVERFLOW`, div-by-zero →
  `E_MONEY_DIVBYZERO`, a non-decimal arg → `E_MONEY_NOT_NUMERIC`, all as JS
  throws that become the reject. Never a silent NaN or wrap.

## Three hosts, one answer (criterion 3)

The load-bearing test: for a spread of `qty × rate` including the `0.335` case,
the **script host**, the **kernel's `decimal.rs`**, and **SurrealDB's decimal**
all agree.

**A representation trap caught this test first — my own just-promoted check.**
I read the script's answer back from the Currency field after the DB round-trip
and string-compared it to the kernel's. It failed: script `"1"` vs kernel
`"1.00"`. Not a disagreement — **SurrealDB strips a decimal's trailing zeros**
(`1.00` → `1`), so I was asserting *representation* where only *value* is
invariant. This is the WO-016 lesson exactly, and I made it again.

The fix is the honest assertion: capture the script's **raw string** off a
`Data` field (text isn't normalized — that is why `"0.20"` and `"1.5625"`
survive there), string-compare hosts 1 and 2, and assert host 3 by **DB decimal
equality** (`out = 1.00dec`, SurrealDB's own comparison — a channel that can't be
accidentally satisfied). String where the string is observable, value where only
value is. That `1.00`/`1` split is itself a recorded finding: the "same decimal
string" property is only observable *off* the money field.

## The scale bug, closed by the one it bit (criterion 4)

The seed's reconciliation script hand-rolled decimal multiply and half-even
rounding in integer minor units. The PM's own first version parsed at a fixed
scale and truncated rate `0.335` to `0.33` → `3 × 0.33 = 0.99`. Rewritten to
the binding:

```js
var amt = Decimal.round(Decimal.mul(lines[i].qty, lines[i].rate), 2, "half_even");
total = Decimal.add(total, amt);
...
if (Decimal.cmp(stated, total) !== 0) throw ... [FRUST:E_INVOICE_UNBALANCED]
```

`3 × 0.335 = 1.005 → 1.00`, exact, and a test asserts the answer **is `1.00` and
is not `0.99`** — naming the wrong answer it must never produce. The seed E2E
still reconciles (49.98 + 1.00 = 50.98) and still refuses an unbalanced invoice.
The hand-rolled `parseDec`/`roundTo`/`centsToString` — ~30 lines of exactly the
arithmetic the platform exists to remove — are deleted.

## The guard is intact (criterion 2, second half)

The binding **adds** a safe path; it does not remove the WO-017 catch. A test
confirms bare JS float math on money (`Number(rate) + 0.2`) is still refused with
`E_MONEY_FLOAT` and still stores nothing, *with the binding present*. Adding
`Decimal` did not open a float back door.

## Containment (criterion 6)

A script calling `Decimal.add` inside `while (true)` is still killed by the
fuel/epoch guard (~520 ms, measured) — the sandbox bounds compute whether or not
the doc touches money. The binding is pure arithmetic; there is no new reachable
surface to escape through.

## Both shared-artifact surfaces

The engine is shared: the kernel runs the wasip2 component, the Desk runs a jco
transpile of it. Rebuilt the component, re-transpiled to the browser core
(`decimal` present in the bundle), redeployed to `frust-desk/assets/engine`. The
browser host runs the identical Boa core, so the binding is present there too.
*Honest bound:* I verified the binding is **present and identical** in the
browser artifact and that the engine still loads and validates; I did **not**
execute a Decimal client-script through a live browser — the three-host proof is
script(kernel-side Boa)/kernel/DB, and the browser runs the same core. A live
browser client-script exercise is a cheap follow-up if wanted.

## Verification

- **`decimal_script_binding` (new, 7 tests):** three-hosts-one-answer, the scale
  bug can't recur, the full API surface exact, mul-doesn't-round, the float catch
  still bites, div-by-zero typed, hot-loop still contained.
- **`accounting_seed_e2e`:** green with the rewritten recon script.
- **Regression:** `decimal_script_catch` (7, the WO-017 guard), `hook_document_fidelity` (4), `hook_dispatch` (4) all green against the new engine.
- Kernel `decimal.rs` unit tests unchanged and green (the source is byte-identical in both targets).

## Files

- `wasm-spike/script-engine/src/lib.rs` — `mod decimal { include!(OUT_DIR) }`, `register_decimal`, the arg helpers, typed throws.
- `wasm-spike/script-engine/build.rs` — the demote-and-copy of the canonical `decimal.rs`.
- `wasm-spike/artifacts/script_engine.wasm` + `browser-engine/` + `frust-desk/assets/engine/` — rebuilt.
- `kernel/tests/decimal_script_binding.rs` — new.
- `kernel/tests/accounting_seed_e2e.rs` — `RECON_SCRIPT` rewritten; the F1 finding comment updated from "future WO" to "closed".

## Scorecard

**P-3.4 bounded→killed** — the canonical bounded, closed by building. Money
correctness is now a clean sweep: decimal at storage, transport, kernel, rollup,
**and script**. Tally **22 killed · 12 bounded · 0 open**.

## Related
[[WO-030 Decimal for Scripts]] · [[2026-07-26 WO-021 money arithmetic]] · [[2026-07-26 WO-022 accounting seed]] (F1, the footgun) · [[ADR-007 Tier-2 Script Architecture]] · [[v1.0 Pain-Point Scorecard]] (P-3.4) · [[assert-outcome-not-operation]]
