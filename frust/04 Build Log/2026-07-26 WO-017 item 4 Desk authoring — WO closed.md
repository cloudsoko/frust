---
tags: [frust, build-log, tier2, sandbox, desk, work-order]
created: 2026-07-26
work-order: "[[WO-017 Client Script Sandbox]]"
status: item 4 of 4 — WO-017 CLOSED
---

# Build Log — WO-017 item 4: Desk authoring · WO closed

The Frappe client-script UX with real isolation underneath, proven as a loop:
**author → runs on next form load → hostile version contained → removed →
2-request posture restored** — every step through the UI, nothing restarted.

## What was built

**Kernel: `POST /doctype/{name}/script`** — attach, replace or clear a
DocType's Tier-2 script. Manager-gated like every metadata write. **No schema
sync runs**: a script is *data* on the metadata record, not shape — this
endpoint is ADR-007's "scripts are live-mutable" made concrete. Script text
goes through `surql::render_value` like every other value; user-authored JS
full of quotes is exactly the payload the validate-then-embed discipline
exists for. Unknown DocType → 404; blank script → cleared (NONE), restoring
the scriptless wire posture.

**Desk: `/script/{doctype}`** (manager) — textarea editor, current script
loaded, save + remove, and the money documentation *in the authoring surface*:
the decimal-safe path (`Number()` → round → `.toFixed(n)`) is shown at the
point of writing, with the rule stated as behaviour ("a float on a Currency
field is rejected, not stored"). The home directory shows `script ●` on
DocTypes that carry one.

The Desk hides the page from non-managers, but **the kernel's `require_manager`
is the enforcement** — hiding is courtesy, not security (WO-008 posture).

## The proof, end to end

| Step | Result |
|---|---|
| Author trim+approval script on `purchase_order` via the UI | saved, `?saved=1` |
| Next form load | engine loads (scripted posture), script runs: `"  Q3 hardware  "` → `"Q3 hardware"` on input |
| Remove script via the UI | next form load is **exactly 2 requests** again |
| clerk1 POSTs to the script endpoint directly | **HTTP 403**, and the DB confirms `client_script: null` — nothing landed |
| Unknown DocType | HTTP 404, no metadata upserted into nowhere |
| Manager authors `while (true) {}` through the legitimate path | 3 kills, breaker trips, **form fully usable**, loud sticky notice |
| Remove the hostile script | cleared; form back to scriptless |

The last two rows are the composition argument: the authoring surface adds no
trust. A hostile script that arrives through the *front door*, saved by a
legitimate manager, is exactly as contained as one injected in a test — because
items 1–3 never asked where the script came from.

## WO-017 closing summary

| Criterion | Where proven |
|---|---|
| 1 — engine outside wasmtime + lazy load | items 1 (partial log: feasibility) — 2 requests scriptless, measured |
| 2 — capability surface probed hostilely | partial log — 7× `undefined`; boundary carried by the artifact |
| 3 — one dialect, two hosts | partial log + item 1 (strip_trace: one rebuild fixed both) + item 3 (identical rejection messages) |
| 4 — decimal NaN-catch | item 3 — five refusal codes, safe path exact, both hosts |
| 5 — worker + watchdog containment | item 2 — spin/hog/user-script killed, breaker, sticky notices |
| 6 — Desk authoring | this log |

**No `eval` fallback path exists.** The engine is the only executor in both
hosts; there is no code path that hands script text to the browser's own
evaluator — the worker imports the engine module and nothing else executes
user text.

ADR-001's three tiers are now all shipped: Tier 0 declarative rules (WO-014),
Tier 1 metadata-driven forms (WO-009/014/015), Tier 2 sandboxed scripts
(this WO).

## Handed to WO-019 (already routed by PM ruling)

- Server-side per-DocType script delivery: the kernel seam exists
  (`load_with_script`, one variable into an empty WASI world) but nothing
  plumbs `client_script` into it — every server-side write still runs the
  engine's built-in default. WO-019 criterion 6.

## Standing findings this WO banked

- module-worker messages before graph evaluation are dropped, not queued →
  the worker announces itself; never sleep-and-hope
- kill notices are sticky; "terminated silently and recovered" is the enemy
  in a success costume
- the engine must never be re-triggered by its own writes (feedback loop:
  `×3` script → `3.7e20`, stopped by the decimal catch)
- "own invocation" for perf gates means "not alongside the running product"
- the realtime tax gate remains a quiet-machine tripwire only; the open
  measurement question is recorded in the item 3 log

## Suite state

**27 binaries green, zero failures** (quiet machine, stack stopped). Scratch
databases dropped at close; dev DB left with `skeleton` only; demo script
restored on `travel_claim`; `purchase_order` left scriptless.

## Related
[[WO-017 Client Script Sandbox]] · [[2026-07-26 WO-017 client script sandbox (partial)]] · [[2026-07-26 WO-017 item 1 browser hosting with lazy-load]] · [[2026-07-26 WO-017 item 2 worker watchdog containment]] · [[2026-07-26 WO-017 item 3 decimal NaN-catch]] · [[ADR-001 UI Extension Tiers]] · [[ADR-007 Tier-2 Script Architecture]]
