---
tags: [frust, build-log, tenancy, security, milestone-4, adr-003]
created: 2026-07-29
status: CHUNK A COMPLETE — db-per-tenant migrated onto the TenancyStrategy seam; bypass guards green (and shown to fail); 277 passed · 0 failed; 3 ms submit / 133.8 req/s / footprint flat
work-order: "[[WO-040 Multi-Tenant Routing]]"
---

# Build Log — WO-040 Chunk A: The Monopoly Seam

The rescoped Chunk A, built to the amended [[ADR-003 Tenancy Model]]:
*tenancy is not a feature; **tenancy topology is a strategy**; resolution is
one operation it performs.* Nothing new is served — database-per-tenant does
exactly what it did — but it can now only be reached one way, and the way is
enforced by a test that fails the build.

## What the seam is

New `kernel/src/tenancy.rs`. Three things, in dependency order:

1. **Typed names with module-private constructors.** `TenantId`,
   `NamespaceName`, `DatabaseName`. `String` is what let a request field
   become a database name; these are what stop it. Validation is one plain
   identifier check in one place, and `a_name_that_is_not_an_identifier_
   cannot_become_a_target` holds the line at `""`, `has space`, `semi;colon`,
   `../etc`, `9leading`, `a'b`, and 65 characters.
2. **`TenancyStrategy`** — `name` / `resolve` / `access_placement` /
   `backup_plan` / `provisioning_plan` / `per_tenant_schema`. One impl so far:
   `DatabasePerTenant`, which is what the kernel already was.
3. **`Tenancy { strategy, conn, registry }`** — the registry is *the only door
   from untrusted input to a target*. `resolve(slug)` looks the slug up, hands
   the **canonical** `TenantId` to the strategy, and mints the
   `ResolvedTenant`.

`ResolvedTenant { tenant_id, namespace, database, environment }` is readable
and unconstructible outside the module. It also carries the connection config
and strategy internally, which is what lets `db.rs` expose the literal
signature ADR-003 asks for.

## The four invariants, and how each is actually enforced

| invariant | mechanism |
|---|---|
| 1. No direct ns/db selection | `ns`/`db` **deleted from the connection config**. `DbConfig` → `ConnConfig { endpoint, root_user, root_pass, access }`. There is no field left to fill in. |
| 2. No shared-context mutation | A `Db`'s target is set at construction and never mutated; `ResolvedTenant` clones are independent. Proven under concurrency, below. |
| 3. Resolution consumes trusted identity | The strategy's `resolve` takes a `TenantId`, and only `Tenancy::register` (startup config) mints one. A raw slug cannot reach a database name. |
| 4. Monopoly enforced structurally | `tests/tenancy_monopoly.rs`, below — and it is shown to fail. |

### The direct-selection sites that are gone

Not just `ns: "frust"` and `cfg.db`. The refactor surfaced three the WO had
not named:

- **`realtime.rs` held a namespace and `use`-d it with whatever tenant string
  a caller passed** — a ns/db selection on the shared WS client, which is the
  textbook shape of a cross-tenant live-subscription bug. `Realtime` no longer
  stores a namespace at all; `subscribe` takes a `&ResolvedTenant`.
- **`sync.rs::KernelConns::acquire` built a fresh config from whatever
  `StorageLocation` the migrator handed it.** It now **checks** the location
  against its own target and refuses a mismatch — an assertion where there was
  a selection.
- **`keyguard.rs` forged its ADR-013 probe token with `db.tenant()` as the
  `db` claim.** Under database-per-tenant that is right by coincidence. With
  the split it would name the wrong database, the probe would be refused for
  the *wrong reason*, and a vulnerable store would report **safe** — the exact
  fail-open the keyguard exists to prevent. Now `database()`, explicitly.

That last one is worth the WO-033 comparison: same class of defect, found by
the refactor rather than by an incident.

### `Db::tenant()` is now `Db::tenant_id()` — not a rename

Every telemetry label, fairness bucket, hook-pool key and metric that said
"tenant" was reading `cfg.db`. Under database-per-tenant those coincide; under
a namespace topology they do not, and a label that means "tenant" must keep
meaning tenant. Splitting them now is what stops Chunk C from silently
re-keying every metric in the system.

## The guard — 11 rules, and it is shown to fail

`tests/tenancy_monopoly.rs`, on the `surql_monopoly` precedent. Needle +
allowlist + reason, over `src/`:

- ns/db on the wire (`surreal-ns`, `surreal-db`, `"ns":`, `"db":`,
  `call("use"`) — transport modules only
- hardcoded constants (`"frust"`, `"skeleton"`) — `tenancy.rs` only
- `StorageLocation {` — projected from the strategy, in `sync.rs` only
- provisioning APIs (`Tenancy::from_config` / `from_env` / `single_tenant(`)
  — startup only, so a request can never register the tenant it then asks for

Plus two structural assertions: **`db.rs` has exactly one `-> Db`
constructor** and it takes a `&ResolvedTenant` (no bare `db()` fallback, which
is the non-negotiable), and **`ResolvedTenant`'s fields are still private**
(publishing them is a one-word change that would restore every bypass at once
and reads as harmless in review).

**`the_guard_catches_a_planted_bypass` runs the same `scan` over one planted
bypass per rule and asserts every one fires**, then asserts it does *not* fire
on the same text inside a comment or in an allowed home. A guard that has
never rejected anything is a green light with no bulb.

It earned it immediately: **the guard's first run failed on three real things
in my own work** — a needle that matched a return type rather than a struct
literal, a rule with no planted case, and a constructor filter that caught
`ConnConfig::default`. Writing it before believing the refactor is why those
were findings and not shipped noise.

## Isolation, proven by provenance, under concurrency

`tests/tenant_isolation_concurrent.rs`. Two tenants, one process, 8 threads,
**one shared `Arc<WasmHooks>`** — the criterion-2 engine is the only
per-request machinery the two tenants now share, so it is where a target would
leak if it could. 48 reads; every returned row asserted by *whose* it is.

The assertions are built so they cannot pass for the wrong reason:

- each tenant's read count must be exactly `threads × rounds` (the run
  happened)
- no read may return zero rows (it read something)
- the maximum read must equal `1 + threads × rounds` — the seed row plus
  **its own** writes and no others (a leak that merely *adds* rows fails on
  the count, not only on the titles)

And a control, `the_provenance_check_can_see_a_foreign_row_when_one_is_really_
there`: a row titled as A's is planted **inside B's database**, and the same
check must find it. Without that, "A never saw B's data" is indistinguishable
from "the read never worked" — which is the [[2026-07-28 WO-039 multi-db
tenancy probe]] near-miss in its other direction.

## Fail-closed config

`FRUST_TENANCY` selects the topology. Chunk A builds `database-per-tenant`;
`single`, `namespace-per-tenant` and `namespace-per-tenant-env` are
**recognised and refused** with "known topology, not built into this binary
yet", and an unknown name is refused as unknown. Silently serving
database-per-tenant for a config that asked for namespace isolation would
leave an operator believing they had a posture they do not have — the same
reasoning as ADR-008's fail-closed boot, and the same reasoning that killed
the "serve anyway" ack.

Boot now says which topology it chose:

```
{"evt":"booted","tenancy":"database-per-tenant","tenant":"skeleton",
 "ns":"frust","db":"skeleton","meta_version":4,"doctypes":1}
```

## Two deliberate deviations from ADR-003's sketch (flagged, not taken quietly)

1. **`resolve` takes a `TenantId` and returns a `TenantPlacement`**, not
   `TenantRequest` → `ResolvedTenant`; `Tenancy` mints the target. This is
   *stronger* on invariants 3 and 4: the strategy never sees untrusted input,
   and the constructor is private to the module rather than merely
   crate-private, so no other kernel module can mint a target even by
   accident.
2. **`backup_plan` / `provisioning_plan` take a `&ResolvedTenant` and cannot
   fail.** Both need placement, so a bare id would mean re-resolving inside —
   and a plan for a tenant nobody resolved is not a question worth being able
   to ask.

Both are in the code comment on the trait. Ratify or reject.

## Numbers — all three M3 wins hold

Suite: **277 passed · 0 failed · 6 ignored, 46 result groups, exit 0** (debug,
full run), plus 5 new `tenancy` unit tests and the 6 new integration tests.

**Submit floor (release, `perf_gates`):**

| gate | budget | measured |
|---|---|---|
| submit warm median | 25 ms | **3 ms** |
| hook chain | 30 ms | **0 ms** |
| realtime tax over 20 parked subs | 2 ms | **0.18 ms** |

**Throughput** (`loadbench`, release, 10 s per rung, dedicated scratch
data-dir `wo040a-load` per standing rule, deleted afterwards; dev store
stopped for the run and restored on its original directory):

| concurrency | WO-026 | **Chunk A** |
|---|---|---|
| 1 | 36.5 req/s | 32.1 (p50 **26.3 ms** vs 26.5) |
| 10 | 123.6 | **133.8** |
| 50 | 120.7 | **135.0** |

0 errors at every rung. The headline **124 req/s is not regressed** — it is
above it. The c=1 rung reads 12% low on throughput while its **p50 is
identical (26.3 vs 26.5 ms)**: at one client, throughput is 1/latency, so this
is tail variance on a single-threaded sample, not per-request cost. If routing
had added work, p50 would have moved.

**Footprint (`tenantmem`, release, 1/10/50 tenants):**

| tenants | 1 | 10 | 50 |
|---|---|---|---|
| RSS | 63.5 MB | 60.9 MB | 65.7 MB |
| per-tenant | 20.5 KB | 2.0 KB | 0.5 KB |

Flat — total does not grow with tenant count, and the per-tenant figure is
**measurement noise divided by N**, not a shrinking cost. P-1.4's tens-of-MB
verdict is intact. (Prior Chunk-A-era run: 76.8 / 66.1 / 63.4 MB.)

## The named coupling — where this lands, stated plainly

`frust serve` still resolves **one tenant at boot** (`Tenancy::sole()`), not
per request. Everything below that line already works for N targets; what is
missing is the router and the per-tenant caches. `SESSION_GEN` and `META_GEN`
remain process-global, so the kernel is **correct but perf-coupled**. Both are
Chunk B, and `sole()` carries the comment saying so.

This is the correct-and-coupled boundary the WO asked for, not a half-routed
tree.

## Files

`kernel/src/tenancy.rs` (new) · `db.rs` (`ConnConfig`, `scoped_db`) ·
`sync.rs` · `app.rs` · `realtime.rs` · `keyguard.rs` · `rest.rs` · `main.rs` ·
`broker.rs`/`worker.rs`/`aggregates.rs`/`boot.rs`/`hooks.rs` (label rename) ·
`tests/tenancy_monopoly.rs` (new) · `tests/tenant_isolation_concurrent.rs`
(new) · ~35 test files migrated mechanically

## Related
[[WO-040 Multi-Tenant Routing]] · [[ADR-003 Tenancy Model]] ·
[[2026-07-28 WO-040 chunk A tenant routing decision]] (the token ruling, now
reconciled under invariant 3) · [[2026-07-28 WO-039 multi-db tenancy probe]] ·
[[2026-07-26 WO-026 surrealdb write concurrency]] (124 req/s) ·
[[2026-07-28 WO-033 kernel hygiene]] (ADR-013 keyguard) ·
[[v2.0 Deployability Gate]] (P-1.4, P-8.1)
