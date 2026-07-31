---
tags: [frust, build-log, tenancy, milestone-4, adr-003, performance]
created: 2026-07-29
status: CHUNK B COMPLETE — two real strategies, boot-time rejection proven on the shipped binary, cache coupling gone; 289 passed · 0 failed; 4 ms submit / 136.7 req/s / footprint flat. ONE NEW FINDING (kernel↔SurrealDB connection churn) and ONE UNSEQUENCED GAP (per-request routing owns no chunk).
work-order: "[[WO-040 Multi-Tenant Routing]]"
---

# Build Log — WO-040 Chunk B: Initial Strategy Parity

Chunk A built the seam and guarded it. Chunk B's job was to prove the seam
has **two real sides** rather than being the old single implementation wearing
a trait — and to remove the cache coupling Chunk A left named.

## 1. Two strategies that genuinely disagree

`SingleTenant` and `DatabasePerTenant`, both behind `TenancyStrategy`. The
distinguishing behaviour is the one thing a tenancy topology exists to decide:

| | two tenants share a database? | restore-one? | schema deploys |
|---|---|---|---|
| `single` | **yes** — tenancy is a label, not a boundary | no | once |
| `database-per-tenant` | no | **yes** | per tenant |

**`BackupPlan` gained an explicit `tenant_isolated` field**, rather than
inferring isolation from "is a database named". Under `single` the unit *is* a
database and the answer is still **no**, because that database holds every
tenant. Inferring would have reported restore-one where [[2026-07-27 WO-027
backup restore DR]] measured restore-all — a wrong answer to the one question
an operator plans DR against.

`DatabasePerTenant::per_tenant_schema()` now truthfully returns `true`. It is
consumed only by `frust_orm::migrate_fleet_with`, which the kernel does not
call, so the answer is currently **inert** — stated rather than left `false`
to avoid thinking about it.

## 2. The abstraction is proven genuine, and the proof is a matrix

`tests/tenancy_strategy_matrix.rs` runs the same assertions over both
topologies from a table, so Chunk C adds a row rather than a test:

```
PASS single              shared_db=true  restore_isolated=false
                         rows(one)=["wo040b_mx_shared-c1"] rows(two)=["wo040b_mx_shared-c1"]
PASS database-per-tenant shared_db=false restore_isolated=true
                         rows(one)=["wo040b_mx_split_a-c1"] rows(two)=["wo040b_mx_split_b-c1"]
```

**The `single` row asserts the uncomfortable thing on purpose.** Both tenants
return *identical* rows, and the test says so rather than skipping the
assertion, because an operator who believes `single` separates their customers
has a far worse problem than one who knows it does not.

**What holds under both is authorization.** A clerk sees only their own rows
in either topology — that is the database enforcing permissions under the
caller's own session (the one-door property), and it is entirely independent
of tenancy. Tenancy and authorization are separate guarantees; the matrix
keeps them separate so neither can be mistaken for the other.

### "Downstream never branches" is enforced, not asserted

Three new rules in `tenancy_monopoly.rs` — `SingleTenant`,
`DatabasePerTenant`, and the literal `"database-per-tenant"` are refused
outside `tenancy.rs`, each with a planted bypass proving the rule fires. The
day `if strategy == "single"` appears anywhere downstream, the build goes red.
That is the order's tell, made structural.

## 3. Boot-time rejection — of config, and of the shipped binary

`Tenancy::from_config` now refuses, before any database contact:

| config | verdict |
|---|---|
| `single` with no database named | **incomplete** |
| `database-per-tenant` **with** a shared database | **contradicts itself** |
| either, with no tenants | **incomplete** |
| a tenant listed twice | **refused** (a duplicate is a typo; de-duplicating it silently hides the mistake) |
| `namespace-per-tenant`, `namespace-per-tenant-env` | known, **not built into this binary** |
| anything else | unknown topology |

The contradiction case is worth its own line: naming a shared database under a
topology that gives every tenant its own means the operator believes something
untrue about their isolation. Ignoring the setting would leave them believing
it.

**And the binary is proven to call it.** `tests/tenancy_boot_refusal.rs`
spawns the real `frust` for every case and asserts a non-zero exit *plus* the
reason in its log — because a validated seam the product never takes is not a
validated product ([[tested-seam-not-wired]] is a standing lesson here, not a
theoretical one). Seven cases, including a **control** that a valid config
exits 0 with `"tenancy":"database-per-tenant"` in the boot line; without it,
six processes exiting 1 would prove only that the binary exits 1.

> **The control earned its keep immediately.** It failed on first run —
> `E_BOOT_DB: The database 'wo040b_boot_ok' does not exist`. The kernel was
> right and my test was wrong: provisioning is a separate act from
> configuring, and ADR-008's fail-closed boot is correct not to conflate them.
> The control now provisions first.

## 4. The cache coupling is gone

New `kernel/src/tenant_gen.rs`. `SESSION_GEN` and `META_GEN` were
process-global `AtomicU64`s, so with N tenants in one process "invalidate on
logout" meant *everyone's* cache — never wrong, but a noisy-neighbour vector
aimed at the caches that took throughput from 15 to 124 req/s.

**Handles, not lookups.** A tenant's generations are `Arc<AtomicU64>` resolved
**once**, when its `Broker` is built, so the per-request path stays a single
atomic load. A `Mutex<HashMap>` lookup on that path would have traded a
cross-tenant coupling for a cross-thread one — the worse deal at 16 workers.
Only invalidation touches the registry, and it is rare by construction.

The session cache is now keyed `(tenant, token)`. Besides making per-tenant
invalidation possible, that stops an auth path resting on 64 random characters
never colliding across tenants.

### The survival proof does not count cache hits

`tests/session_cache_per_tenant.rs`. Counting hits would prove nothing an
off-by-one could not fake, so the instrument is the session row itself:

1. A and B both log in and read (caches warm, provenance checked).
2. **B's session row is deleted out of band.** From here B's token resolves
   *only* from cache — a miss goes to the database and finds nothing.
3. A logs out.
4. B still works, and still returns **B's own row**. ⇒ A's logout did not
   touch B's cache.

And the control, `the_same_instrument_shows_b_failing_when_b_is_the_one_
invalidated`: same deletion, but B's *own* generation is bumped — B then
**401s**. That is what stops step 4 from meaning "invalidation is broken
everywhere".

## 5. Numbers

Suite: **289 passed · 0 failed · 6 ignored, 48 result groups, exit 0**, plus
`tenancy_boot_refusal` (7 tests) run separately.

**Submit floor (release):** submit warm median **4 ms** (gate 25) · hook chain
**0 ms** (gate 30) · realtime tax **0.04 ms** (allowance 2).

**Footprint (`tenantmem`, 1/10/50):** 63.7 / 61.2 / 63.1 MB. Flat — two
`Arc<AtomicU64>` per tenant do not register.

**Throughput** (`loadbench`, release, 10 s per rung, dedicated scratch
data-dir, 45 s settle between rungs, dev store restored afterwards):

| concurrency | WO-026 | Chunk A | **Chunk B** |
|---|---|---|---|
| 1 | 36.5 | 32.1 | **43.2** (p50 22.8 ms) |
| 10 | 123.6 | 133.8 | **135.9 / 136.7** (two runs) |
| 50 | 120.7 | 135.0 | **135.8** |

0 errors at every rung. The per-request atomic load costs nothing measurable.
The c=10 rung was run **twice** because of the instrument failure below.

**Chunk A's low c=1 reading (32.1) was tail variance, now confirmed:** the same
rung re-measured at 43.2 req/s with p50 22.8 ms. Recording it here rather than
leaving the earlier caveat as the last word on it.

## 6. FINDING — the instrument failed, and it named a real constraint

The first throughput pass returned `c=10 → 3.3 req/s`. It was not reported as
a result, because it cannot be one: **1252 requests, zero errors, p50 62.1 ms,
p99 139.5 ms**. Healthy latencies; the arithmetic implied 379 s of wall time
for 10 s of work. A client thread had failed to join for ~6 minutes.

Diagnosis before reporting, in order:

1. `/health` answered in **64 µs** while the bench was "stalled" — the kernel
   was idle and available, so the stall was client-side.
2. The kernel log stopped growing entirely: nothing was queued.
3. `netstat`: **3600 sockets in TIME_WAIT, 3537 of them to :8899** (SurrealDB)
   and only 60 to :8790 (the kernel). The ephemeral pool was exhausted by the
   *kernel's own database connections*, so the load generator could not get a
   port to reach the kernel.
4. Stopping the kernel dropped TIME_WAIT to 461 within 30 s, draining cleanly
   to 115 — live churn, not a leak.

Same class as the [[2026-07-28 WO-032 SSE retire polling]] `ECONNREFUSED`: the
load generator failed, not the thing being measured. The re-run with settle
pauses gave the stable numbers above.

**The constraint it named, which is a property of the kernel and not of this
chunk:** under sustained load the kernel opens roughly **one TCP connection to
SurrealDB per query**, so TIME_WAIT accumulates at request rate — measured
here climbing 118 → 1252 → 4274 → 6150 across three 10 s benches. On Windows
the ephemeral range is ~14 000 ports shared with everything else on the box,
so a long enough run at this rate would exhaust it. It did not affect any
number reported above, and it is **pre-existing** (WO-024/025/026 all ran
against it — it is what the perf-hygiene rules have implicitly been working
around). Flagged for sequencing, not fixed here: connection reuse in `db.rs`
is a change to the WO-026 hot path and wants its own measured order.

## 7. THE GAP — per-request routing owns no chunk

Stated plainly because nothing else in the board says it:

- Chunk A named "one tenant per process" as its landing boundary.
- Chunk B removed the **cache** coupling, not that one.
- The rescoped Chunk C is namespace topologies.

So **`frust serve` still resolves one tenant at boot and no chunk owns making
it route per request.** Rather than leave that implicit, `main` now *enforces*
it: a roster of more than one tenant **refuses to boot** ("this binary serves
one tenant per process; refusing rather than serving a subset") instead of
silently serving the first name. `Tenancy::sole()`'s comment says the same.

The limit is honest and loud. It still wants sequencing.

## Files

`kernel/src/tenant_gen.rs` (new) · `tenancy.rs` (`SingleTenant`,
`TenancyConfig`, validation) · `broker.rs` (per-tenant generations on the
`Broker`) · `rest.rs` (`(tenant, token)` cache key) · `sync.rs` · `main.rs`
(roster + boot-time refusal) · `tests/tenancy_strategy_matrix.rs` (new) ·
`tests/session_cache_per_tenant.rs` (new) · `tests/tenancy_boot_refusal.rs`
(new) · `tests/tenancy_monopoly.rs` (+3 rules)

## Related
[[WO-040 Multi-Tenant Routing]] · [[ADR-003 Tenancy Model]] ·
[[2026-07-29 WO-040 chunk A tenancy seam]] ·
[[2026-07-26 WO-026 surrealdb write concurrency]] (the caches, the 124 req/s) ·
[[2026-07-27 WO-027 backup restore DR]] (restore-one = restore-all) ·
[[2026-07-28 WO-032 SSE retire polling]] (the same instrument-failure class) ·
[[v2.0 Deployability Gate]] (P-1.4, P-8.1)
