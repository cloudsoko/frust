---
tags: [frust, build-log, benchmark, production, scale, work-order]
created: 2026-07-26
work-order: "[[WO-024 Load and Footprint Benchmark]]"
status: complete — the knee found, the memory numbered, two scorecard rows re-scored
---

# Build Log — WO-024: Load & Footprint Benchmark

Pure measurement. The two unmeasured runtime claims from the v1.0 scorecard now
have numbers, and the concurrency suspicion is confirmed as hard as a
measurement can confirm anything.

## Method

- Release `frust` + release `loadbench` driver (threaded `ureq`, one OS thread =
  one concurrent client — the honest model; an async client would hide queueing
  behind its own scheduler).
- **1 M-row dataset resident** (`sales_invoice`, 1,001,790 rows, ns `scale`) in
  the same `surreal.exe` the kernel writes through — so RSS includes the real
  dataset (criterion 2's resident condition). Store: `frust-scale-data` (a
  dedicated fixture, not the dev store — caveat honored).
- Real submit path: `POST /write/thing` (auth session → plugin+script hooks →
  DB write), login once, token shared.
- Warm-up run discarded; each rung 8-12 s.

## THE KNEE — there isn't one, and that IS the finding

| concurrency | throughput | p50 | p95 | p99 |
|---|---|---|---|---|
| 1 | **16.1 req/s** | 61 ms | 72 ms | 78 ms |
| 10 | 15.0 | 653 ms | 723 ms | 736 ms |
| 50 | 13.5 | 3.7 s | 3.8 s | 3.8 s |
| 200 | 13.4 | 11.0 s | 22.1 s | 23.3 s |
| 500 | 15.0 | 21.9 s | 32.5 s | 45.2 s |

**Throughput is flat at ~13-16 req/s from 1 to 500 clients. Latency scales
linearly with concurrency** (p50 ≈ concurrency × the 61 ms single-client service
time). This is the textbook signature of a single-threaded serializing
bottleneck: adding clients buys no parallelism, they queue behind a one-at-a-time
handler, so each waits behind all the others.

There is no concurrency knee because there is **no concurrency benefit at all** —
the ceiling is at concurrency 1.

### The bottleneck, profiled not guessed

The serve loop is structurally single-threaded (confirmed in code:
`recv_timeout` takes ONE request, `handle` runs it to completion — auth, hooks,
write — before the loop accepts the next; no thread fan-out). The **CPU signal
proves it is the ceiling**: under 500 clients, `frust serve` used **5.1 s of CPU
over 6 s of wall — ~0.85 of a single core, and never more**. A single thread
maxes one core; the kernel does exactly that and cannot go faster no matter how
many clients wait. SurrealDB is **not** the bottleneck — it sits at ~1 GB RSS,
idle, able to serve concurrent queries the serialized kernel never sends it.

Secondary serializer (named, not measured separately): even if the accept loop
were multi-threaded, `HookInstance` is a `Mutex`, so the hook stage would
serialize next. WO-026 must address both.

Sub-finding: single-client REST submit is **61 ms**, vs the perf gate's ~25 ms
broker-internal path. The ~36 ms delta is HTTP + a per-request session-token
lookup (a DB round trip on every authenticated request) + the 1 M-store
substrate. The session lookup per request is a candidate for the WO-026 pass.

## THE MEMORY NUMBER (P-1.4, the lone OPEN → KILLED)

| condition | frust serve | surreal.exe | combined |
|---|---|---|---|
| idle, 1 M resident | **59.9 MB** | 961 MB | 1021 MB |
| peak, 500-client load | **76.1 MB** | 1002 MB | 1078 MB |

**The kernel is 60 MB resident, growing to 76 MB under 500 concurrent clients —
no per-connection memory explosion**, one process. Frappe holds 300-500 MB
**per worker** and needs many workers; Frust holds 60 MB, once. SurrealDB's
~960 MB is the 1 M-row dataset — the data itself, which any database pays
(MariaDB would too). The lone OPEN closes to a decisive **KILLED**, which is
exactly what the open verdict existed to allow: measure, then score.

## Scorecard re-scored (this WO was licensed to)

- **P-1.4 open → killed** — 60 MB measured kernel footprint.
- **P-1.1 bounded, bound now hard** — flat ~15 req/s, single-threaded ceiling,
  named as the **#1 production blocker**: until the serve loop is
  multi-threaded, Frust's raw concurrency is *worse* than Frappe's multi-worker
  model. Honest, and it sequences the milestone.

New tally: **20 killed · 14 bounded · 0 open**.

## Did NOT fix (discipline)

No optimization here — a fix smuggled into a benchmark is how you stop trusting
the benchmark. **WO-026 fixes what this found**, measured against these numbers:
multi-thread the serve loop, address the hook-instance mutex, and consider
caching the per-request session lookup. The before is on the record above.

## Hygiene

Dev store untouched (bench ran on the `frust-scale-data` fixture). The bench
wrote ~1,400 rows into a `thing` table in the fixture's `frust` ns; the fixture
is rebuildable and the 1 M `scale` data is untouched. `loadbench` is a committed
`src/bin/` driver for the WO-026 before/after.

## Related
[[WO-024 Load and Footprint Benchmark]] · [[v1.0 Pain-Point Scorecard]] · [[Frappe Pain Points]] (P-1.1, P-1.4) · [[2026-07-23 SurrealDB week-1 benchmark]] · [[2026-07-24 WO-006 1M-row scale proof]]
