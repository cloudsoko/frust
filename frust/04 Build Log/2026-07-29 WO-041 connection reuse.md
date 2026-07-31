---
tags: [frust, build-log, surrealdb, performance, production, milestone-4]
created: 2026-07-29
status: COMPLETE — TIME_WAIT flat at 0 across a 3-minute soak (~26k requests, 0 errors) where the old model would have burned ~26k sockets; throughput UP, not traded
work-order: "[[WO-041 Connection Reuse]]"
---

# Build Log — WO-041: SurrealDB Connection Reuse

The production-load ceiling WO-040 Chunk B surfaced, closed. The kernel opened
**one TCP connection per query**; it now reuses a pooled one per endpoint.

## Probe first — is the connection even reusable?

A client-side pool is worthless if the server hangs up after every response,
so that was checked before any code changed. SurrealDB 3.2.0's `/sql` reply
carries **no `Connection: close`** — HTTP/1.1 keep-alive by default. The
connection was always reusable; nothing server-side had to change, and the
churn was entirely ours.

## The A/B that settled the mechanism

`examples/connprobe.rs`, 200 sequential queries against the dev store:

| model | TIME_WAIT left behind | rate |
|---|---|---|
| bare `ureq::post` *(what `db.rs` did)* | **200** | 54.8 req/s |
| one persistent `ureq::Agent` | **1** | **63.7 req/s** |

One socket per query, exactly as the Chunk B finding predicted — and the fix
is **not a trade**. Reuse removes a TCP handshake per query, so it is faster
*and* bounded. The escalation clause asked for a number if a pool regressed
throughput; there was no regression to report in either direction of concern.

## What changed

`db.rs` gains `agent_for(endpoint) -> ureq::Agent` — a process-global registry
keyed by **endpoint**, and `Db` resolves its agent **once at construction**, so
the query path never touches the registry. That shape is deliberate and is the
WO-024/025 lesson applied: *don't trade a resource win for a lock.* The pool is
sized to the REST worker pool (`clamp(2,16)`) **plus 8** headroom for the
resident worker, rollup drains and boot, so 16 workers in flight never evict
each other's connection and fall back to a handshake apiece.

**Per endpoint, not per tenant** — every tenant's queries go to the same
SurrealDB, so pool size follows query *rate*, not tenant *count*. N tenants add
no sockets, which is what keeps this orthogonal to WO-040.

**One caller outside `db.rs` was fixed rather than excepted.** `broker.rs`'s
legacy WO-002 `ExternalHookRunner` also used the bare free function. It is not
on a hot path, but "one agent per endpoint" is a process-wide rule and an
exception there would be a port leak waiting for the day someone moves it onto
one. It now goes through the same registry.

## The proof

### Criterion 1 — sustained load, TIME_WAIT flat

**A 3-minute soak** (six overlapping 30 s load runs at c=10, ~26 000 requests,
**0 errors**), sampling TIME_WAIT against :8899 every 30 s:

```
t=0s 1 · t=30s 1 · t=60s 1 · t=90s 1 · t=120s 0 · t=150s 0 · t=180s 0 · final 0
```

**Flat at zero.** Under the old model this run would have left roughly one
socket per query — ~26 000 against a ~14 000-port range, i.e. exhaustion
partway through.

**The soak was checked for vacuity while running**: the kernel log grew 2 791
lines in 6 s and **22 connections sat ESTABLISHED** to :8899 (pool size 24).
That is the mechanism visible directly — connections *held open and reused*
rather than cycled into TIME_WAIT — and it rules out "flat because nothing was
happening", which is the way this measurement would most naturally lie.

The 0 errors across ~26 k requests also answers the one risk a pool introduces:
a reused connection the server had quietly closed would surface as a transport
failure, and none appeared over three minutes.

### Criterion 2 — the WO-026 hot path is not regressed

Throughput at c=10, six 30-second samples (longer than the usual 10 s, since
this WO is about *sustained* behaviour):

| | samples | median |
|---|---|---|
| **WO-041** | 133.2 · 142.3 · 146.8 · 140.9 · 143.7 · 156.2 | **143.0** |
| WO-040 Chunk C | 138.0 · 140.1 · 134.4 · 125.8 · 138.7 | 138.0 |
| WO-026 baseline | — | 123.6 |

Every sample above the 124 baseline, and the median **improved** — consistent
with removing a handshake per query. p50 fell to 62.7–72.7 ms.

Submit floor (release): **3 ms** (gate 25) · hook chain **0 ms** (gate 30) ·
realtime tax **0.25 ms** (allowance 2).

The WO-026 caches and the conflict-retry are untouched by construction — this
changed *how the transport connects*, not what it sends — and the full suite
covers both: **309 passed · 0 failed · 6 ignored, 51 result groups, exit 0**,
including `session_cache_per_tenant`, `meta_cache_invalidation` and
`conflict_canary`.

### Criterion 3 — no serialization under the 16-worker pool

The 22 ESTABLISHED connections during the soak are the direct evidence: workers
held their own connections concurrently rather than queueing on one. Throughput
went **up** and p50 **down** under the same concurrency, which is the opposite
of what a contended shared client produces.

### Criterion 4 — footprint held

`tenantmem` at 1/10/50 tenants: **65.6 / 65.4 / 67.2 MB**. Unchanged class; the
pool is per-endpoint so tenant count does not multiply it.

## The regression guard

`tests/connection_reuse.rs`, two halves because either alone is weak:

1. **Behavioural** — 200 real queries must not leave a socket apiece
   (threshold `< QUERIES/4`, an order of magnitude from both outcomes so
   background churn cannot move the verdict). Measured: **0 new sockets**.
2. **Structural** — no module in `src/` may use the bare `ureq` free
   functions. This catches the regression the day it is written rather than
   the day it exhausts a production box's ports. It found the `broker.rs`
   caller above on its first run.

**The failure mode stays reproducible**, per the `naive-blocking-sse`
precedent: `cargo run --release --example connprobe -- bare 200` demonstrates
the old behaviour on demand. A metric never seen to fail is not yet a metric.

## Reconciled with WO-026, not contradicting it

WO-026 measured `bare ≈ fresh ≈ pooled within 10%` **on throughput** and was
right. The resource dimension was never measured. Both hold: per-request
connections are throughput-neutral *and* port-exhausting under sustained load.
Another instance of *measure the dimension you are claiming, not an adjacent
one* — and this time the adjacent measurement was our own, two milestones back.

## Files

`kernel/src/db.rs` (`agent_for`, `Db::agent`, four transport sites) ·
`kernel/src/broker.rs` (legacy hook runner through the same registry) ·
`kernel/examples/connprobe.rs` (new — the A/B and the reproducible control) ·
`kernel/tests/connection_reuse.rs` (new — behavioural + structural guards)

## Related
[[WO-041 Connection Reuse]] · [[2026-07-26 WO-026 surrealdb write concurrency]]
(the model this revisits, and the throughput it must not regress) ·
[[2026-07-29 WO-040 chunk B strategy parity]] (where it surfaced) ·
[[2026-07-26 WO-025 concurrent serve loop]] (the 16 workers) ·
[[2026-07-26 WO-024 load and footprint benchmark]] · [[v2.0 Deployability Gate]]
