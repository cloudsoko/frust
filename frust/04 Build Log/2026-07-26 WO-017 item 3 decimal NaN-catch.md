---
tags: [frust, build-log, tier2, sandbox, money, work-order]
created: 2026-07-26
work-order: "[[WO-017 Client Script Sandbox]]"
status: item 3 of 4 complete — WO still open; two escalations
---

# Build Log — WO-017 item 3: The decimal NaN-catch

Money that a script corrupts is now refused, loudly and typed, in **both
hosts** — one rebuild, per the pattern item 1 proved. Plus the approved
instrument fix, which did not end where it was expected to.

## The catch

The WO-001 shell already recorded each field's WIT variant kind on the way in
and re-imposed it on the way out. The catch is that shell **declining to
re-impose what a script corrupted** — no new mechanism, the existing one
finally saying no.

| Script does | Result | Code |
|---|---|---|
| `Number(doc.amount) + 0.2` → `0.30000000000000004` | refused, **nothing stored** | `FRUST:E_MONEY_FLOAT` |
| `Number("not a number")` → NaN | refused | `FRUST:E_FIELD_NAN` |
| `doc.amount = {total: 5}` | refused | `FRUST:E_MONEY_TYPE` |
| `doc.amount = "twelve euros"` | refused | `FRUST:E_MONEY_NOT_NUMERIC` |
| `v.toFixed(2)` (the safe path) | accepted, **exactly 30.30** | — |
| `doc.amount = 42` (integral) | accepted | — |
| `doc.amount = null` (clear) | accepted | — |

Seven kernel tests (`decimal_script_catch.rs`), and the two load-bearing cases
re-verified in the browser against the same artifact — **identical message,
character for character**, field left untouched, zero containment strikes
(a rejection is not a kill).

**NaN is caught before `JSON.stringify`**, because stringify turns NaN into
`null` — indistinguishable from a script deliberately clearing a field. One
step later and "corrupted" and "cleared" are the same value. Catching it in the
epilogue keeps clearing legal and corruption loud.

### The rule, and why it is shaped this way

A decimal field may leave a script as an exact **string**, an **integral
number**, or **null**. Never as a fractional number — a fractional JS number
*is* a float and carries float error.

Integral numbers are allowed because they are exactly representable and
round-trip exactly. Fractional ones are refused *even when exact* (`10.5` is,
`10.1` is not), because a rule an author can apply without knowing which is
which is worth more than a rule that is usually right.

**The documented safe path:** money arrives as a string; `Number()` it, round
explicitly, write it back with `.toFixed(n)`. Exact money arithmetic belongs on
the server (REQ-6.2.2 / WO-020) — a script should route, flag and label money,
not compute it.

## Findings

### 1. The kernel cannot run per-DocType scripts at all

`WasmHooks` built its guest world with `WasiCtxBuilder::new().build()` — an
empty environment — and **nothing in the kernel plumbs a DocType's
`client_script` anywhere**. Every server-side write has been running the
engine's *built-in default* (`validate.js`), for every DocType, since WO-001.

The empty environment is right and must stay: inheriting the kernel's env would
hand a sandboxed guest every secret the process holds. What was missing is the
*deliberate* seam. Added `WasmHooks::load_with_script`, which supplies exactly
one variable (`FRUST_SCRIPT`) into an otherwise empty world — the kernel-side
counterpart of the browser's `_setEnv`.

**This is a seam, not a feature.** Per-DocType server-side script delivery
remains unbuilt, and it is a genuine hole in ADR-007's Tier-2 story that
predates this WO. Flagged for an order of its own.

### 2. A script that derives a field from itself fed on its own output

`applyDoc` dispatches `input` so Topcoat's signals track what the script
decided — which re-triggered the run. For an idempotent script that converges
and hides; for a self-referential one it compounds every pass. A `x3` rule took
`10.10` to `3.7e20` in about a dozen cycles, and **the decimal catch is what
stopped it** — the runaway ended as `E_MONEY_NOT_NUMERIC` when `toFixed`
returned exponential notation.

Item 1's demo script was idempotent, which is why this never appeared until a
script computed money. The engine is now never re-triggered by its own writes;
user edits still trigger normally.

## ESCALATION 1 — the realtime tax gate, after the approved fix

Implemented as ruled (microseconds + A/B/A/B interleaving) **and** three further
confounds found and removed along the way:

- **paired** per-round differences, not the difference of pooled medians —
  pooling first discards what interleaving buys
- **one engine compile instead of twelve** — `submit_batch_us` built a broker
  per batch, and `broker()` calls `WasmHooks::load`, which compiles the 4 MB
  engine. My own interleaving had made every batch a cold start. Removing it
  cut runtime 165 s → 41 s and was the single largest noise source
- **counterbalanced ordering** and a **per-batch table reset** — every batch
  writes rows, so the table grew monotonically and the later condition paid for
  it

**It still does not reproducibly resolve a 2 ms allowance.** Readings at budget
20 across the session: 0.53, 0.89, 1.29, 2.67, 2.93, 3.09, 3.23, 3.59, 3.60,
4.16, 4.76, 5.01 ms.

The decisive diagnostic: **lowering the budget 20 → 12 made the measured tax go
UP** (5.31 / 3.80 / 4.66). A real per-subscription cost cannot do that. At
budget 8 it read 4.75 / 1.87 / 2.13 — no scaling with subscription count at
all.

**Neither published number was moved.** I set the budget to 12, then to 8, and
reverted both: no reading earned a change to a published property. The
condition in the ruling — "if the fixed instrument still reads above 2 ms, the
budget comes down" — is not satisfied, because the fixed instrument is not yet
measuring subscriptions.

**My own methodological error, named:** every failing reading today was taken
with the Desk running and Chrome hosting a 4 MB engine. The full suite, run
with both stopped, passes the gate. WO-016's "perf gates get their own
invocation" was applied as "not alongside other tests" and should have meant
"not alongside the running product". That is now a standing caveat.

What remains genuinely open: this gate infers a ~1 ms cost from end-to-end
latency of a DB round-trip on a shared developer machine. That may simply not
be measurable here, and the honest options are a quiet reference machine or a
different measurement entirely (counting DB-side work rather than timing it).
A design question, not something to improvise inside item 3.

## Suite state

**27 binaries green, zero failures** (the 27th is the new
`decimal_script_catch`), on a machine with the Desk and browser stopped. The
realtime tax gate flaps under background load, per the escalation above.
Scratch databases dropped at close.

## Related
[[WO-017 Client Script Sandbox]] · [[2026-07-26 WO-017 item 2 worker watchdog containment]] · [[2026-07-26 WO-016 decimal rollup accumulation]] · [[ADR-007 Tier-2 Script Architecture]] · [[ADR-011 Realtime]] · [[SRS]] (REQ-6.2.1, REQ-6.2.2, REQ-6.1.2)
