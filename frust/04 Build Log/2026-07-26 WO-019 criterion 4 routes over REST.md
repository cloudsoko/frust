---
tags: [frust, build-log, apps, plugins, routes, work-order]
created: 2026-07-26
work-order: "[[WO-019 App Lifecycle]]"
status: criteria 1-4 done — WO active
---

# Build Log — WO-019 criterion 4: Routes over REST

The door probe's in-process equality now runs **through the shipped dispatch
path** — real HTTP, real bearer tokens, real throttle policy — with bearer
auth, session lookup, trace spans, tenant attribution and WO-013's budget all
sitting between the handler and the broker.

## The equality survived the wiring

That was the danger the PM named, and it is the acceptance test:

```
route body: ok:[{"id":"purchase_order:haw8…","title":"clerk1 order","total":"115"}]
```

Asked once over HTTP through `/app/demo/probe` and once directly of the broker,
**same caller, same question**: same row count, one row, clerk1's own. No
`clerk2 order` anywhere in the response. The assertion is `via_route.len() ==
via_broker.len()` computed in the same test against a live server, so drift in
dispatch code fails the build rather than quietly widening a result set.

The five hostile probes were re-run **over HTTP**, because dispatch code
between guest and broker is exactly where a bypass would be introduced:

| probe | over HTTP |
|---|---|
| raw doctype | `refused: E_UNKNOWN_DOCTYPE { name: "purchase_order; REMOVE TABLE purchase_order" }` |
| raw filter | `refused: FRUST:E_BAD_FILTER … expected one of and/or/not/cmp` |
| identity table | `refused: E_UNKNOWN_DOCTYPE { name: "app_user" }` |
| own socket | `refused: Permission denied` |
| filesystem | `refused: No such file or directory` |

## A plugin route is not a budget bypass

```
call 1 -> 429 {"error":{"kind":"tenant-throttled","retry_after_ms":956}}
```

WO-013's `E_TENANT_THROTTLED` fires through `/app/{app}/{path}`, with its retry
hint intact. **Proven by tripping it**, not by inspection.

The throttle is admitted at **dispatch**, in addition to the broker door the
handler's own verbs already pass. Two reasons, both load-bearing:

1. **Running a wasm handler is real compute even if it never reads.** A route
   that did no db work would otherwise be free, and "free" is how a budget gets
   bypassed.
2. **The guest picks its own status code.** Left to the handler, a throttled
   call surfaces as whatever the plugin felt like returning.
   `E_TENANT_THROTTLED` has to be the *kernel's* answer, not a plugin's opinion
   about one — the test asserts the **429 comes from the kernel**.

Dispatch order is the criterion: **throttle → enabled → resolve → run.**

## Traces name whose code ran

`route_dispatch` spans carry `app` as a field, and `frust_route_duration_ms` is
labelled by app, path and tenant. WO-010's reconstruction property therefore
extends to third-party surface **on day one** rather than being retrofitted
once someone's plugin is slow: a trace through a plugin route says *whose* code
ran, not merely that guest code ran.

## Also proven

- **Bearer discipline** — no token, no route: `401 E_UNAUTHENTICATED`. A plugin
  route is not a public endpoint.
- **Disable reaches routes** — a disabled app stops serving, and says why; it
  serves again after enable. Criterion 3's detach covers the route surface, not
  just scripts.
- **Unknown app / unknown route** → clean `404`, not a 500.

## Reserved route names

`/app/{app}/{path}` shares its shape with the lifecycle verbs, so `enable` and
`disable` are **refused as route paths at manifest validation**. Match ordering
already protects the lifecycle verbs; without the validation rule a plugin
could declare a route that silently never fires. Refusing at validation beats
shadowing for the rest of the app's life.

## Component cache

Compiling a component per request would make a plugin route's latency a
compilation benchmark, so compiled hosts are cached per process, keyed by
filename. Safe because a `RouteHost` holds no per-app state — app identity and
caller both arrive per call, and each call still gets a **fresh store** whose
door is torn down with it.

## Open question for the PM (small, but it is a contract)

A **disabled** app's route currently answers **400** (`InvalidValue` →
"app 'demo' is disabled"). 400 means "malformed request", and the request is
not malformed — the app is off. 404 (not currently served) or 409 (state
conflict) both read truer. The test asserts *not-200 and says why* rather than
a specific code, so this can be settled without touching the test. Flagging
rather than changing, because HTTP status codes on a third-party-facing surface
are a contract.

## Finding — I repeated a WO-013 mistake, and the suite caught it

`app_routes_e2e` passed in isolation under `--test-threads=1` and **failed in
the suite**, three tests at once: a concurrent read got the throttle test's
`429`, and two others got the disable test's *"app 'demo' is disabled"*.

Cause: the file shares one kernel, one installed app and the process-global
tenant policy, and two tests mutate that shared state deliberately. Cargo runs
a binary's tests in parallel by default, so they sabotaged each other.

This is exactly the collision WO-013 found in the fairness unit tests, made
again. The lesson that generalises, and the reason it is recorded here rather
than quietly fixed: **passing under `--test-threads=1` proves nothing about the
suite.** Serializing is the fix (same mutex the permission proofs and perf
gates use); running in *default parallel mode* is the check.

A related self-inflicted break preceded it: adding the `app` parameter to
`RouteHost::handle` broke criterion 1's `door_probe` call site, so the "suite"
run had actually been a compile failure. `cargo test --no-run` before a long
background run costs seconds and distinguishes "it failed" from "it never ran".

## MAJOR SUBSTRATE FINDING — the dev store degrades ~3x over a working day

The suite came back **30 green, 1 failed**, the failure being `perf_gates`.
Chasing it produced the most useful measurement of the session.

`gate_submit_latency` read **84 ms against a 60 ms gate** — 3x the 25-27 ms
this machine produced all day — and it did so *after* dropping 58 scratch
databases and *after* restarting `surreal.exe`. Neither helped, which ruled out
both the WO-016 accumulation finding and process state.

The store is `surrealkv://` file-backed, and its data directory is **a single
50 MB append-only WAL with no compaction** — every write from ~500 database
create/drop cycles plus a day of tests, in one file. A restart does not shrink
it (still 50.4 MB afterwards).

Definitive experiment — same binary, same code, data directory moved aside,
fresh store, then restored:

| Gate | bloated store | fresh store |
|---|---|---|
| submit median | **84 ms** — FAIL (gate 60) | **34 ms** — pass |
| hook chain | 3 ms | **0 ms** |
| realtime tax | **5.43 ms** — FAIL | **0.00 ms** — pass |
| wall time | 124 s | 61 s |

**Nothing regressed. The substrate rotted.**

### This substantially resolves the open realtime-tax escalation

The tax gate has been flapping across 0-6 ms all session, and I could not
explain it after removing four separate instrument confounds. On a clean store
it reads **0.00 ms** with that same interleaved, paired, counterbalanced,
µs-sampled instrument.

It also explains the anomaly that stopped me lowering the subscription budget:
*"budget 20 → 12 made the measured tax go UP"*. It would — the reading was
tracking WAL growth during the run, not subscription count. **The instrument
was probably fine by then; the ground under it was moving.** Which is exactly
why the budget was left at 20 and neither published number moved.

### The rule this produces

Perf gates need a **fresh store**, not merely a quiet machine and a dropped
scratch DB. "Own invocation" now has a third clause:

> not alongside other tests · not alongside the running product · **not on a
> store that has absorbed a day of churn**

Dev data was restored intact afterwards (six doctypes, `skeleton` present).

## Suite state

`app_routes_e2e` — 6 tests green **in default parallel mode**, 13.9 s.
Full workspace: **30 binaries green, 1 failed** — the failure being
`perf_gates` on the bloated store, which passes **3/3 on a fresh one**.

## Next

Criterion 5 (honest uninstall — the written answer, with the detach machinery
already built and exercised), criterion 6 (server-script delivery, routed here
from WO-017 item 3), criterion 7 (the demo app end to end, no restarts).

## Related
[[WO-019 App Lifecycle]] · [[2026-07-26 WO-019 criterion 1 the door probe]] · [[2026-07-26 WO-019 criterion 3 the install story]] · [[ADR-006 Plugin Capability Surface]] · [[SRS]] (REQ-2.2.2, REQ-6.4.1) · [[WO-013 Tenant Fairness]]
