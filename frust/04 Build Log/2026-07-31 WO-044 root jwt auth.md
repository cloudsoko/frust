---
tags: [frust, build-log, performance, security, surrealdb, milestone-4]
created: 2026-07-31
work-order: "[[WO-044 Root JWT Auth]]"
status: COMPLETE — all 4 criteria met, correctness and ADR-013 gated before any number. **The 124 req/s ceiling was an argon2 ceiling.** Paired saturation sweep: Basic saturates ~117 req/s, JWT ~555 req/s (4.7× at c=8, 7.3× at c=2). Per root query 16.53 ms → 297 µs (55.6×, 5 samples/arm, converged). One signin for 614 root calls in the live kernel. ADR-013 intact — the re-signin retry is scoped to root-only calls BY DESIGN, because putting it in the shared path would have made the keyguard's forged probe succeed and report every healthy store compromised.
---

# WO-044 — Root JWT for `sql_root`: killing the per-request argon2 tax

Escalated out of [[2026-07-30 WO-043 email batteries]] finding 1, ruled its own
order, built here.

## The shape of the fix

`sql_root` authenticated every call as Basic `root:root`, and SurrealDB
argon2-verifies that password **per request**. Now the kernel signs in once,
caches the root JWT per endpoint, and sends a Bearer token — argon2 runs once at
signin, and the hot path is a signature check.

| | |
|---|---|
| new state | `RootAuth { cached: Option<CachedRoot>, basic_only: bool }`, one per `endpoint\|user` |
| resolved | once at `scoped_db()`, exactly like WO-041's `agent_for` — the registry lock is never on the query path |
| hot path | one uncontended `RwLock` read + a `String` clone |
| refresh | at 75% of the token's **own** `exp`–`iat` (measured 1 h → refresh at 45 min) |
| backstop | a rejected token re-mints and retries **once** |
| escape hatch | `FRUST_ROOT_AUTH=jwt\|basic`, unknown value refuses the boot |
| fallback | `/signin` unavailable → Basic, **loudly** (error log + `frust_root_auth{mode="basic"}`) |

Probed before designing, not assumed: a root JWT carries `ID: root` with **no
`ns`/`db` claims**, and was verified to work for database-scoped DDL, `DEFINE`/
`REMOVE DATABASE` at namespace scope, and `INFO FOR KV`. Scope-free is why one
token serves every tenant on an endpoint — N tenants mint one token, not N.

## Criterion 1 — correctness, before any number

`tests/root_jwt_auth.rs`, 7 tests, all landed before the benchmark ran:

- **identical results** across the real root surface (projection SELECT, ordered
  SELECT, aggregate, DDL, CREATE, UPDATE, read-back, DELETE, `sql_root_raw`),
  JWT arm vs Basic arm, same database, compared value-for-value.
- **argon2 runs once**: 25 root queries, ≤1 signin — asserted on a *per-handle*
  counter, not a process-global one, so a parallel test cannot perturb it.
- **a rejected token re-signs in and retries once**: a planted bad token must
  never reach the caller, and the retry must return the *real* answer (3 users),
  not an empty one.
- **refresh lands before expiry**, derived from the token's own claims and
  asserted against them (`refresh_in < life` and `> life/2`).
- wrong root credentials still fail.

## Criterion 2 — ADR-013 integrity

**ADR-013's subject is the record-access signing key; WO-044 changes only how
the kernel authenticates as root. The two are disjoint** — the ADR text makes no
claim about root credentials at all. They meet at exactly one place, the shared
`sql_with_auth`, and that is precisely where the retry was kept out:

> The keyguard probes with a **deliberately forged** Bearer token and reads the
> resulting 401 as its healthy answer. Had the re-signin retry gone into
> `sql_with_auth` — the obvious place — that forged probe would have been
> retried as root, succeeded, and the guard would have reported **every healthy
> store compromised**: a boot-refusing false positive on every deployment.

So `with_root_retry` wraps only calls the kernel makes as itself, and
`the_root_retry_never_upgrades_a_caller_supplied_credential` asserts a forged
token is still refused with a 401 *while the kernel is perfectly capable of
minting a working one*. `the_boot_guard_still_refuses_a_redacted_key_store`
installs the placeholder key for real and asserts the typed refusal still fires,
so the guard is not green for the wrong reason.

`keyguard_canary` 4/4 and `kernel_hygiene`'s two keyguard tests stay green.

**Posture, stated plainly as the WO asked:** a cached root JWT is a bearer
secret held in process memory that **expires in an hour**. The kernel already
holds `root_user`/`root_pass` in memory for the life of the process and sends
them on every request. The token is strictly narrower in time and is not the
password. No path ADR-013 closes is opened.

## Criterion 3 — the win, measured

**Primary A/B** — `examples/rootauth.rs`, against a **dedicated scratch store**
on :8901 with its own data dir (never the live dev store's, per standing
policy; the example refuses a `:8899` endpoint outright). Both arms in one
process, same database, interleaved, 5 samples × 30 queries:

```
JWT   median 297.3µs   (samples 261.6µs … 331µs)     spread 1.27×
BASIC median 16.5281ms (samples 15.7082ms … 18.3394ms) spread 1.17×
→ 55.6× faster, 16.23 ms saved per root query
→ signins across all handles: 1  (for 314 root queries)
```

**The throughput answer the WO asked for.** Paired concurrency sweep, quiet
machine, kernel restarted per arm:

| concurrency | Basic req/s | JWT req/s | ratio | Basic p50 | JWT p50 |
|---|---|---|---|---|---|
| 1 | 45.2 | **327.4** | 7.2× | 21.0 ms | 3.0 ms |
| 2 | 75.5 | **550.5** | 7.3× | 26.5 ms | 3.5 ms |
| 4 | 91.6 | **563.8** | 6.2× | 42.6 ms | 7.1 ms |
| 8 | 116.7 | **552.6** | 4.7× | 66.0 ms | 14.4 ms |

Basic saturates at **~117 req/s** — which is the historical **124 req/s**
number. **That ceiling was an argon2 ceiling.** JWT saturates at **~555 req/s**
and is now bound by something else. Earlier c=10 pairs on a busier machine
agreed on the ratio (608.9/611.7 vs 131.4 → 4.6×; 478.7/466.0 vs 94.2 → 5.0×).

**WO-043's notification floor, re-measured:**

```
before: no rule 2.91ms | rule + HEALTHY 50.95ms | rule + DEAD 39.29ms   (root RTT 16.5ms)
after : no rule 2.91ms | rule + HEALTHY  7.19ms | rule + DEAD  6.02ms   (root RTT  485µs)
```

The rule-attached overhead fell **48.0 ms → 4.28 ms**. WO-043's criterion 2
(healthy ≈ dead) still holds, now at a floor 7× lower.

**The submit floor does NOT move** — 2 ms in both modes, measured twice each.
Reported as a negative result because it is one: the submit path writes as a
record user over an already-cached session Bearer, and WO-026 had already cached
the metadata reads. Root auth was never on it.

## Criterion 4 — no hot-path cost reintroduced

Asserted as an outcome rather than by inspection: the live kernel serves **614
root calls on 1 signin** (`frust_root_signin_total 1`), and the benchmark run
completes 314 root queries on 1. No argon2 on the request path; no registry lock
on the request path; the token is a `RwLock` read resolved from a handle fixed at
construction.

## Findings

### 1 · A write costs ~3 root queries, and nobody had counted them

Added `frust_db_calls_total{kind=root|session}` — and **labelled by credential
class, not by header scheme**: the pre-existing span used
`auth.starts_with("Basic")`, which WO-044 would have silently turned into "every
root call is a session call". A metric quietly measuring the wrong thing is
worse than no metric.

Measured two ways that agree: **3.05** root calls per `/write` (loadbench, 1612
writes) and **2.77** (60 writes with the idle background rate subtracted),
against exactly **1** session call — the actual write. At 16.5 ms apiece that is
~50 ms of argon2 per request, which *is* the old 124 req/s ceiling.

**Which three has not been attributed** — that is open follow-up, not a claim I
am making. `load_server_script` (uncached, per validate) is the leading suspect.

### 2 · An idle kernel issued 19 root queries/second

Measured over a 6-second idle window: the resident worker tick, rollup drains and
the mail worker together made **114 root calls in 6 s**. Under Basic that was
**~314 ms of argon2 per second — roughly a third of a core, permanently, on a
completely idle kernel**. That cost is now ~5 ms/s.

### 3 · `frust_root_auth` published nothing in Basic mode (mine)

The gauge was only set where a token was minted, so `FRUST_ROOT_AUTH=basic`
produced **no series at all** — leaving an operator unable to distinguish
"running on Basic" from "this kernel predates the metric". An absent series is
not a reading. Fixed and verified in both modes.

### 4 · `kernel_hygiene` has a pre-existing parallel-execution flake

The sweep caught `revoke_kills_every_session_of_one_user_immediately` failing
with `revoked = 3` where the test creates 2 clerk sessions. **Not WO-044:**

| | failures |
|---|---|
| `FRUST_ROOT_AUTH=jwt` | 2 / 5 |
| `FRUST_ROOT_AUTH=basic` (pre-WO-044 path) | 3 / 5 |
| `--test-threads=1` | **0 / 5** |

Identical value, identical assertion, both auth modes — and it disappears
entirely when serial. The test asserts an **exact global count over shared
mutable state** (one kernel, one database, four tests in one binary running
concurrently), which cannot be reliably correct under parallel execution.
Diagnosed and reported, deliberately **not fixed here** — it is off-order.

## Instrument failures, all mine

1. **I predicted the write path had no root queries** (WO-026 caches metadata,
   the write is a session Bearer). The throughput A/B contradicted it instantly —
   131 vs 610 req/s. The metric I then added found ~3 per write. The prediction
   was reasoned and wrong; the measurement is why it did not reach the log as a
   fact.
2. **A per-endpoint attribution that measured the background thread.** My first
   per-route probe reported `/health` costing "4 root calls" — a route that
   touches no database. It was measuring a wall-clock window that included
   background maintenance. The re-run subtracts an idle baseline; without the
   `/health` control the `/write` number would have looked plausible and been
   ~50% noise.
3. **A `timeout … | grep` harness that reported "Terminated" as a kernel hang.**
   Three throughput runs looked like the kernel wedging at c=10; the same run
   with a redirect instead of a pipe completed in 4 s at 462.7 req/s, zero
   errors. I very nearly recorded a nonexistent concurrency bug.
4. **I reached for WO-040's port-exhaustion diagnosis and was wrong.** The c=10
   hang looked exactly like the TIME_WAIT finding, so I checked before
   asserting: 234 TIME_WAIT total, 5 to the kernel. A prior WO's lesson is a
   hypothesis, not an answer.
5. **A test fixture that was not idempotent**, and a comparison that included
   SurrealDB's own `time` stopwatch string — both made the parity test fail on
   things that were not differences.

## Regression

Full kernel sweep: **every binary green** except the pre-existing
`kernel_hygiene` parallel flake above — including `surql_monopoly`,
`tenancy_monopoly` 4/4, `keyguard_canary` 4/4, `boot_discipline`,
`identity_hardening`, `worker_queue`, `rest_surface`, `perf_gates` 3/3 (submit 2
ms vs a 25 ms gate), `mail_notifications` 6/6, plus 20 more.

Browser: `pnpm workflow` 18/18 · `pnpm sse` 8/8 · `pnpm mail` 15/15, all against
the WO-044 kernel through `frust serve`.

## Dev-store mutations (stated)

- `app_user:u` + doctype `thing` seeded into `skeleton` — the WO-024 loadbench
  fixture, which the live store never had. **Left in place** so the benchmark is
  re-runnable.
- ~43,800 `thing` rows written by the benchmarks — **deleted** (`DELETE thing`,
  verified 0).
- The scratch store on :8901 and `D:\Dev\rust\wo044-scratch` were created for
  the primary A/B and **removed** afterwards.
- Kernel restored to default (`FRUST_ROOT_AUTH` unset → jwt) with the WO-043
  mail config.

## Related
[[WO-044 Root JWT Auth]] · [[2026-07-30 WO-043 email batteries]] (surfaced it) ·
[[ADR-013 Signing-Key Integrity at Boot]] · [[2026-07-26 WO-026 surrealdb write
concurrency]] · [[2026-07-25 WO-024 load and footprint benchmark]] ·
[[2026-07-28 WO-033 revoke endpoint]] · [[v2.0 Deployability Gate]]
