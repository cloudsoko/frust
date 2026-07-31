---
tags: [frust, build-log, tenancy, milestone-4, adr-003, disaster-recovery, scorecard]
created: 2026-07-29
status: CHUNK C COMPLETE — four topologies, all through one seam; P-8.1 KILLED with executed evidence. TWO FINDINGS: namespace-level RECORD access is impossible on SurrealDB 3.2.0 (probed), and `surreal import` is additive, not restore-over.
work-order: "[[WO-040 Multi-Tenant Routing]]"
---

# Build Log — WO-040 Chunk C: Namespace Topologies

The tenancy arc closes. One binary whose topology is a config value:
`single`, `database-per-tenant`, `namespace-per-tenant`,
`namespace-per-tenant-env` — all through the one guarded seam Chunk A built.

## FINDING 1 — the probe that reshaped the chunk before it was built

The order anticipated that "namespace topologies **may** place `DEFINE ACCESS`
ON NAMESPACE". **They may not.** Probed against SurrealDB 3.2.0 before any
design was committed:

| statement | result |
|---|---|
| `DEFINE ACCESS … ON NAMESPACE TYPE RECORD` | **HTTP 400** |
| `DEFINE ACCESS … ON NAMESPACE TYPE JWT` | OK |
| `DEFINE ACCESS … ON DATABASE TYPE RECORD` *(control)* | OK |

RECORD access is database-scoped, and it has to be: the kernel's entire auth
model is `SIGNIN (SELECT * FROM app_user …)` — a query against a **table**,
and tables live in databases. So `AccessPlacement::Namespace` is unreachable
for the kernel's own signin, and **all four topologies answer `Database`**.

Two consequences worth stating rather than burying:

1. Under `namespace-per-tenant-env` this is a **security gain**. A separate
   access per database means a separate signing key per environment: a sandbox
   token cannot authenticate against production even though both databases sit
   inside the tenant's own namespace.
2. `AccessPlacement::Namespace` is **kept, not deleted**. The question is real
   and namespace-level *JWT* access does work, so a future design where the
   kernel issues its own tokens could use it. Nothing returns it today, and
   `provision.rs` **refuses** to render it rather than emitting DDL SurrealDB
   would reject — the line that would have to be designed is marked, not
   guessed.

### And the fail-open stays closed

`boot()` now asks the strategy for its access placement and **refuses to boot**
if the answer is anything the meta DDL cannot honour. The reason is the Chunk A
keyguard finding: DDL written for one location plus an ADR-013 probe aimed at
another means the probe is refused for the *wrong reason* and a compromised
store reports **safe**. `the_keyguard_probes_the_right_place_under_a_namespace_
topology` proves it where tenant id and database genuinely differ (tenant
`wo040d_kg_a`, database `app`) by decoding the probe token's claims.

## The two new topologies

| strategy | namespace | database | isolates tenants? |
|---|---|---|---|
| `namespace-per-tenant` | **the tenant** | configured (`app`) | yes |
| `namespace-per-tenant-env` | **the tenant** | **the environment** | yes |

**`DeploymentEnvironment` is a closed enum** — `Production | Sandbox | Test`.
Invariant 3 extended to the environment axis: a raw string never becomes a
database name, so `FRUST_ENVIRONMENT=prod-2` is refused at boot instead of
silently creating a database nobody provisioned and no backup plan knows about.

**A process is pinned to one environment and can address no other.** Serving
prod and sandbox from a single process would make "a routing bug wrote prod
data from a sandbox request" a *reachable state*; separate processes make it
unreachable. Same no-surface reasoning that shaped the rest of this seam.

### Every unused setting is a contradiction

Extending Chunk B's discipline to the new axes:

| supplied | under | refusal |
|---|---|---|
| `FRUST_NS` | either namespace topology | "the namespace IS the tenant" |
| `FRUST_DATABASE` | `namespace-per-tenant-env` | "the database IS the environment" |
| `FRUST_ENVIRONMENT` | `single` / `database-per-tenant` | "no environment axis" |
| *(nothing)* | `namespace-per-tenant` | "no database named" |
| *(nothing)* | `namespace-per-tenant-env` | "does not know whether it is production" |

## The matrix, four rows

`namespace-per-tenant` is the row that earns its place: **both tenants'
databases are called `app`**. Placement is therefore asserted on the
`(namespace, database)` **pair**, never the database name alone — a check
comparing names would call two perfectly separated tenants "the same
database". Row titles moved from database name to **tenant id** for the same
reason: it is the only label that stays meaningful across topologies that
reuse database names.

```
PASS single                   ns_shared=true  db_shared=true  restore_isolated=false
PASS database-per-tenant      ns_shared=true  db_shared=false restore_isolated=true
PASS namespace-per-tenant     ns_shared=false db_shared=false restore_isolated=true
                              one=("wo040d_ns_a","app") two=("wo040d_ns_b","app")
PASS namespace-per-tenant-env ns_shared=false db_shared=false restore_isolated=true
                              one=("wo040d_env_a","sandbox") two=("wo040d_env_b","sandbox")
```

Authorization holds under all four — a clerk sees only their own rows whatever
the topology, because that is the database enforcing permissions under the
caller's session. Tenancy and authorization stay separate guarantees.

## FINDING 2 — `surreal import` is additive, not restore-over

Found by **running** the ops path rather than asserting a plan. Importing onto
live data fails:

```
ERROR Database record `ledger:fa507bhi9e5nvxy5nunq` already exists
```

WO-027 and WO-039 both imported into a **fresh** database and so never met
this. The real per-tenant restore is therefore **three steps, not two**:

> **export → drop the target database → import**

Anyone who scripts export→import without the drop has a restore that fails
**halfway through an incident** — which is precisely when nobody is in a
position to debug it. The drop is scoped to the unit the strategy named, so it
stays a per-tenant operation.

## P-8.1 — KILLED, with executed evidence

`tests/tenant_restore_ops.rs`, run against `namespace-per-tenant` (the
topology where "the tenant" and "the database" are least alike):

1. The plan comes from the **strategy**: `backup_plan` → `--ns wo040d_dr_a
   --db app`, `tenant_isolated: true`.
2. `surreal export` that unit.
3. **Mutate both tenants after the export** — A corrupted, B given a new row —
   so "restored" and "untouched" are claims with teeth.
4. Drop A's database (finding 2), import.
5. **A is back at `a-original`; B still holds `b-original` *and*
   `b-written-after-export`.**
6. Provenance: every row A holds is owned by A.

**Control:** `the_shared_topology_refuses_to_promise_a_tenant_isolated_restore`
— `single` must report `tenant_isolated: false`, so the assertion above is
reading a real answer and not a constant that happens to be `true` everywhere.

**Verdict: P-8.1 bounded-by-architecture → KILLED.** Restore-one is
operational through the strategy for all three isolating topologies, whole-
instance is `resolve_all()` + per-tenant export, and the three-step procedure
is recorded. The one topology that cannot promise it says so.

## Numbers

Suite: **307 passed · 0 failed · 6 ignored, 50 result groups, exit 0.**

**Submit floor (release):** submit warm median **4 ms** (gate 25) · hook chain
**0 ms** (gate 30) · realtime tax **0.00 ms** (allowance 2).

**Footprint (`tenantmem`, 1/10/50 tenants):** 76.2 / 66.0 / 65.4 MB. Total does
not grow with tenant count; the 1-tenant reading is the high one, which is the
first-run allocator warm-up this harness has shown before and not a per-tenant
cost. P-1.4's tens-of-MB verdict holds.

**Throughput** (`loadbench`, release, 10 s per sample, dedicated scratch
data-dir, 50 s settle between samples, dev store restored afterwards).
**≥3 samples per rung, per the banked lesson** — a single sample is not a
measurement:

| concurrency | samples | median | WO-026 baseline |
|---|---|---|---|
| 10 | 138.0 · 140.1 · 134.4 · 125.8 · 138.7 | **138.0** | 123.6 |
| 50 | 143.3 · 146.6 · 157.3 | **146.6** | 120.7 |

0 errors at every sample, and **every individual sample at both rungs is above
the 124 req/s baseline** — so the conclusion does not depend on which one you
read. The c=10 spread (125.8 to 140.1) is exactly why the rule exists: read
alone, the low sample would have looked like a 10 % cost for namespace
topologies that measurement shows is not there.

**One rung is deliberately absent.** `c=1` is not reported: five samples across
Chunks A/B/B2 spanned 26.5 → 43.2 → 26.7 req/s, a ~60 % swing, because at one
client throughput is `1/mean` and the mean is tail-dominated. It carries no
signal at this sample size, and quoting it either way would be dressing noise
as evidence.

## Files

`kernel/src/tenancy.rs` (`DeploymentEnvironment`, `NamespacePerTenant`,
`NamespacePerTenantEnvironment`, four-way config validation) ·
`kernel/src/provision.rs` (new — plan→DDL, `provision`, `provision_all`,
`export_target`; added to the `surql_monopoly` allowlist by ruling: every
value it interpolates is a typed `NamespaceName`/`DatabaseName` whose
constructor is private to `tenancy`) · `boot.rs` (access-placement refusal) ·
`tests/tenancy_strategy_matrix.rs` (4 rows, placement by pair) ·
`tests/tenant_restore_ops.rs` (new — the P-8.1 evidence + the keyguard proof) ·
`tests/tenancy_boot_refusal.rs` (namespace topologies inverted from "not
built" to "boots and reports where")

## Related
[[WO-040 Multi-Tenant Routing]] · [[ADR-003 Tenancy Model]] ·
[[2026-07-29 WO-040 chunk A tenancy seam]] (the keyguard finding this keeps
closed) · [[2026-07-29 WO-040 chunk B strategy parity]] ·
[[2026-07-29 WO-040 chunk B2 per-request routing]] ·
[[2026-07-27 WO-027 backup restore DR]] (P-8.1's origin; the redacted-key
finding still applies to every restore) · [[2026-07-28 WO-039 multi-db tenancy
probe]] · [[v2.0 Deployability Gate]] · [[WO-041 Connection Reuse]]
