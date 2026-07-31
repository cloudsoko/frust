---
tags: [frust, build-log, concurrency, surrealdb, caching, work-order]
created: 2026-07-26
work-order: "[[WO-026 SurrealDB Write Concurrency]]"
status: complete — all 5 criteria; P-1.2 re-earned KILLED; 15 → 124 req/s across the milestone
---

# Build Log — WO-026: SurrealDB Write Concurrency

The probe answered a question the WO's own fork did not anticipate, and the
answer was better news than either branch.

## Criterion 1 — the probe, and a third outcome

A four-cut design, each cut eliminating one explanation:

| hypothesis | probe | verdict |
|---|---|---|
| surrealkv single-writer wall | raw writes, pooled agent, rungs 1/10/50 | **refuted** — 39.9 → 144.0 → 152.0 w/s, it scales |
| kernel connection model | `bare` (what `db.rs` does) vs `fresh` vs `pooled` | **refuted** — all within ~10%; a pool buys nothing |
| our table shape (events, changefeed, permissions, asserts) | kernel-shaped table vs bare, same run | **refuted** — 147.6 vs 143.7 w/s, identical within noise |
| **kernel per-request round trips** | code trace + arithmetic | **CONFIRMED** |

**The SurrealDB adoption bet is not at its scale limit** — the feared
pillar-level finding did not materialise. Every authenticated write was making
**three sequential DB round trips**: `session_caller` (session lookup),
`load_doctype` (metadata), and the write itself. Raw ceiling ~150 round
trips/s ÷ 3 ≈ 50 req/s; measured 45-48. **The arithmetic matched the
measurement to the digit**, which is what turned a hypothesis into a verdict.

### The finding that outranked the WO

`load_doctype` running a `SELECT` on every request **is P-1.2 — "metadata
loaded per request"** — the Frappe pain point this project criticized and then
reproduced. Killed on latency (a sub-ms indexed read is not Frappe's 50-200 ms
deserialize), but the *mechanism* was ours too. Invisible while single-threaded;
one third of the ceiling the moment WO-025 parallelized the loop. **The
scorecard was downgraded to bounded before the fix was written**, not after.

## Criterion 2 — invalidation is the work, not the caching

Nine invalidation tests, written and green **before a single throughput number
was claimed**, because a cache serving stale metadata or a revoked principal is
the WO-020 Finding-A silent-wrong class in the paths where it is catastrophic.

**Metadata cache — generation, not TTL.** Every kernel-mediated metadata write
bumps a global generation; an entry is reused only while its generation
matches. Staleness is *structurally impossible* rather than improbable. Bump
sites: `POST /doctype`, app install/update (`attach_metadata`), uninstall, and
schema sync. Proven: new doctype visible immediately · re-sync exposes an added
field · app install usable at once · **uninstall refuses from cache** (the
dangerous direction) · cache keyed per doctype so one cannot answer for another.

**Session cache — belt and braces, because it is a permission path.** Logout
bumps a generation that drops *every* entry; a 5 s TTL backstops expiry and
out-of-band revocation. Proven: **logout refuses the very next request**, with
no TTL wait · other sessions survive by re-reading rather than becoming
collateral damage · **a hit is byte-identical to a miss** (this one caught a
real JWT-reseed bug that would have silently broken the row-permission half on
request two) · unknown tokens still refused, so the cache is not an auth bypass.

### Why coarse invalidation, stated as a principle

Coarse (drop everything on logout) beats fine (per-token removal) here because
**the failure modes are asymmetric**: a forgotten bump site in the coarse design
costs hit-rate; a forgotten removal site in the fine design leaves a revoked
token live — a silent auth hole in the one path this WO exists to protect.
Choosing the strategy whose worst-case bug is a performance regression instead
of an auth bypass is how a correctness budget should be spent. Upgrade trigger:
per-token invalidation **only on measured evidence of cache thrash** — you do
not accept a more dangerous failure mode to fix a problem you do not have.

**The stated trade (PM-ratified):** out-of-band revocation — an operator
deleting the session row directly, bypassing the kernel — takes effect within
5 s rather than instantly. The common path (logout) is the instant path. Queued
as a v1.1 battery: a first-class kernel revoke endpoint that bumps the
generation, making admin-forced revocation instant too and demoting the TTL to
pure safety net.

## Criterion 3 — throughput rises again, and the new bound

| rung | WO-024 | WO-025 | **WO-026** |
|---|---|---|---|
| 1 | 16.1 req/s | — | **36.5** |
| 10 | 15.0 | 45.1 | **123.6** |
| 50 | 13.5 | 40.4 | **120.7** |
| 200 | 13.4 | 42.6 | **106.9** |
| c=1 p50 | 61 ms | — | **26.5 ms** |

**15 → 48 → 124 req/s: 8.2× across the milestone**, with the arithmetic
tracking the round-trip prediction at every step (3 trips → 48, 2 → 73, 1 →
124, against a raw ceiling of ~150).

**The new bound, named:** the kernel now runs at ~80% of raw SurrealDB write
throughput. Frust is **DB-bound rather than self-limited** — the remaining gap
is the write round trip itself. Beating ~150 w/s is an *architecture*
conversation (batching, sharding, ADR-003's per-tenant process), not more
kernel tuning. The kernel has given what kernel tuning can give.

## Criterion 4 — every guarantee intact

**37 test-result groups across 36 binaries, 0 failed, exit 0**, with both
caches live. The atomic job claim, ADR-012's permission-refused-write typing,
the changefeed, and the docstatus lattice all hold.

## Criterion 5 — floor and footprint

Fresh store, dedicated scratch dir, quiet machine:

- **submit warm median 5 ms** (gate 60; REQ-6.1.1's release floor is 25 ms) —
  down from 32 ms, because the metadata round trip is gone from the uncontended
  path too. The floor is now beaten **five-fold**.
- hook chain 0 ms · realtime tax 0.00 ms
- **RSS 75.4 MB under 200 concurrent clients** — *lower* than WO-025's 84 MB at
  2.6× the throughput. Faster completion means fewer in-flight requests means
  less resident state: **the concurrency win paid a memory dividend rather than
  charging one.** P-1.4 comfortably killed.

## Scorecard

**P-1.2 killed → bounded → killed, in one day.** Both halves of the gate met:
the per-request reads are gone (by construction and by the 3→2→1 arithmetic)
*and* invalidation held (nine tests + full suite). Tally: **21 killed · 13
bounded · 0 open**.

## Related
[[WO-026 SurrealDB Write Concurrency]] · [[2026-07-26 WO-025 concurrent serve loop]] · [[2026-07-26 WO-024 load and footprint benchmark]] · [[v1.0 Pain-Point Scorecard]] · [[SurrealDB]] · [[ADR-003 Tenancy Model]] · [[ADR-012 Row-Write Permission]] (the silent-wrong class this cache had to avoid)
