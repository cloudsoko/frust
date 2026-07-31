---
tags: [frust, build-log, tier2, sandbox, containment, desk, work-order]
created: 2026-07-26
work-order: "[[WO-017 Client Script Sandbox]]"
status: item 2 of 4 complete — WO still open; one escalation open
---

# Build Log — WO-017 item 2: Worker + watchdog containment

The ADR-005 spike bar, met in the browser: **hostile loop and allocation bomb
terminated, form alive, error loud and stripped.** Proven hostile-first, with a
genuinely hostile *user script* and not only the spike's exports.

## The design tension, resolved and reported

**A Web Worker does not break the single-document posture.** It is a thread,
not a document — same origin, no iframe, no navigation, no second page. It
costs one request, on scripted forms only.

What it *does* change is real: **`validate` stops being synchronous.** That is
not a preference. There is no browser mechanism to interrupt a synchronous loop
on the main thread, so an engine that runs there cannot be contained at all —
not "less contained", *uncontainable*. `Worker.terminate()` is the browser's
only forcible stop, and it needs no cooperation from the code being killed.
Containment is therefore the reason for the worker, not a benefit of it.

## The containment proof

| Probe | Result |
|---|---|
| `spin()` — infinite loop | killed at the call budget, form editable, main thread responsive, worker respawned |
| `hog()` — allocation bomb | killed, form editable, worker respawned |
| **hostile user script** (`while (true) { n = n + 1; }`) | 3 kills, breaker tripped, form fully usable — **self-limiting in 12 s with no user interaction** |
| 3rd strike | script disabled for the page, worker not respawned, form still works |

Healthy-path cost, measured: **`validate` = 0.3–0.4 ms**, against a 250 ms call
budget — roughly 800× margin, so the watchdog cannot fire on honest work.

## What was built

- **`/engine/worker.js`** — the cell. Deliberately dumb: no policy, no budget,
  no retry count. *A watchdog that lives inside the thing it watches is not a
  watchdog.*
- **`boot.js`** is now the supervisor: budgets, kill, restart, circuit breaker.
- **Circuit breaker at 3 strikes.** Without it the watchdog becomes its own
  denial of service — a permanently hostile script would spawn, load 4 MB and
  die on every keystroke forever.
- **Coalescing**: one call in flight at a time; keystrokes during a run queue a
  single re-run rather than piling up.

Budgets: init 15 s (covers compiling 4 MB of WebAssembly), call 250 ms. Both
measured, not guessed.

## Findings

### 1. Messages to a module worker before its module graph evaluates are DROPPED

Not queued — dropped. Measured in Chrome: an immediate `init` never arrives; the
identical `init` after a delay does. The engine imports fine in a worker (1.5 s),
so this presents as a silent hang with no error, no console output, and nothing
on the network tab.

Fixed by making the **worker announce itself** (`{t:"up"}` as the last statement
of module evaluation); the supervisor waits for that before sending `init`. The
obvious alternative — sleep and hope — would have converted a deterministic
failure into an intermittent one on slower machines, which is the same bug
wearing a disguise.

### 2. A kill notice that erases itself is not "loud"

First implementation terminated the allocation bomb correctly and then **wiped
its own error message** ~1.5 s later, when the respawned worker validated
successfully. "Terminated silently and recovered" is precisely the silent
misbehaviour this project treats as the enemy — and it only showed up because
the hog probe was checked on a longer timeline than the spin probe.

Kill notices are now **sticky**: a success cannot erase one; only a newer
message of the same severity replaces it.

### 3. Scratch databases — fixed at source (the rider)

`orm/src/testkit.rs` provisioned `orm_t_<pid>_<n>` databases and removed the
previous run's on the way *in*, so every invocation left a fresh set resident
forever — the unbounded source. `TestDb` now removes its database on `Drop`,
best-effort so a failed cleanup can never redden a passing test or panic while
unwinding. `attach()` handles are marked non-owning so extra connections do not
drop the database out from under the owner.

**Verified: 95 orm tests now leave zero databases behind.**

Honest remainder: the 14 kernel test files each roll their own fixed-name
scratch DB (`REMOVE ... IF EXISTS` then `DEFINE`), so a full suite leaves **34
databases** — bounded and reused, never growing. The ritual is over; the
residue is constant. Left as-is rather than churning 14 files.

## ESCALATION — the realtime tax gate cannot resolve its own allowance

`gate_submit_with_live_subscriptions` failed. It is **not** a regression, and I
have changed neither `LIVE_SUB_BUDGET_PER_TABLE` nor `LIVE_TAX_BUDGET_MS`.

Five consecutive isolated runs, clean substrate:

| run | baseline | with 20 subs | tax |
|---|---|---|---|
| 1 | 27 ms | 30 ms | 3 ms |
| 2 | 27 ms | 30 ms | 3 ms |
| 3 | **43 ms** | **30 ms** | **0 ms** |
| 4 | 28 ms | 30 ms | 2 ms |
| 5 | 25 ms | 31 ms | 6 ms |

Allowance is 2 ms. Observed spread is 0–6 ms. **Run 3 measured the
with-subscriptions case as 13 ms *faster* than its own baseline** — the
instrument is reading noise, not tax.

Two concrete causes, both in the instrument:

1. **Quantization.** Samples are collected with `as_millis()`, truncating every
   sample to a whole millisecond. A 2 ms allowance is being read off a ±1 ms
   instrument on a ~27 ms base.
2. **The gate's stated premise does not hold.** Its comment says "machine drift
   moves both halves together, so the gate judges realtime's cost and nothing
   else." That is only true if drift is slow relative to the measurement. Run 3
   shows drift moving the halves *apart* by 13 ms — the drift is faster than the
   measurement, so it does not cancel.

Tested and **rejected**: scratch-DB accumulation as the cause (34 present →
dropped to 1 → same 3 ms result).

The gate's own instruction is "lower the budget, do not widen the allowance",
and that instruction is right — which is exactly why this must not be answered
by touching either number while the instrument is this blunt. Proposed remedy,
for ruling rather than for me to improvise: measure in **microseconds**, and
**interleave** the two conditions (A/B/A/B) so drift cancels *within* a run
instead of across its halves. Neither changes a published property; both make
the same assertion honestly. WO-016 already flagged this gate as sitting at its
limit — it has now crossed it in both directions.

## Suite state

**25 binaries green.** `perf_gates` is the 26th and is the escalation above —
`gate_hook_overhead` (0 ms / 30) and `gate_submit_latency` (25–27 ms / 60) pass
comfortably; only the realtime tax delta flaps. Scratch databases dropped at
close.

## Related
[[WO-017 Client Script Sandbox]] · [[2026-07-26 WO-017 item 1 browser hosting with lazy-load]] · [[ADR-005 Plugin Isolation]] · [[ADR-007 Tier-2 Script Architecture]] · [[ADR-011 Realtime]] · [[SRS]] (REQ-6.1.2, REQ-6.5)
