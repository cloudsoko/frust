---
tags: [frust, build-log, tenancy, security, milestone-4, adr-003]
created: 2026-07-29
status: CHUNK B2 COMPLETE — one process, N tenants, routed per request; isolation provenance-proven over the SHIPPED HTTP path and spoofing refused; 299 passed · 0 failed; 4 ms submit / footprint flat
work-order: "[[WO-040 Multi-Tenant Routing]]"
---

# Build Log — WO-040 Chunk B2: Per-Request Routing

The WO's own title, executed: **one process, N tenants.** Chunk A built the
seam, Chunk B proved the strategies and un-coupled the caches — and still the
kernel resolved *one* tenant at boot and refused a roster larger than one.
This is the piece that makes a request find its own tenant.

## The mechanism (the ratified option-(a) ruling, under ADR-003 invariant 3)

Recorded here as a security-shaped decision, per WO-008 precedent.

**1. `/login` is told which tenant.** It has no token yet, so it must be — the
second bootstrapping wall the original decision record named. Order of hints:

| hint | source |
|---|---|
| explicit | a `tenant` field in the login payload |
| implicit | the request's subdomain (`acme.frust.app` → `acme`) |
| fallback | **only when exactly one tenant is registered** |

Two deliberate properties. The fallback exists **only where it cannot be
wrong** — with one tenant there is nothing to guess between, which is what
keeps the Desk and every existing client working unchanged; with two it
returns nothing and login refuses rather than picking. And a hint that **is**
present but does not resolve is refused outright, never falling through to the
sole tenant — otherwise a wrong subdomain would quietly log someone into
somewhere else.

`subdomain_of` is deliberately conservative: `127.0.0.1:8790`, `localhost`,
`localhost:3000`, `frust.app` and `::1` all yield **no** tenant. Inventing one
from an octet or a port would be a routing decision made out of noise.

**2. The kernel mints `<TenantId>.<random>`** after authenticating in that
tenant's database. The prefix is the **canonical `TenantId` the registry
produced** — kernel-asserted after validation, never the client's string
handed back. The random half is `rand::string(64)`, re-checked alphanumeric
before it is stored.

**The row stores only the secret.** It already lives in the tenant's own
database, so a prefix inside it would be redundant — and a lookup that also
matched the prefix would be checking the client's claim against itself.

**3. Every authenticated request splits its token before any database call.**
Both halves are validated before anything reaches a query: the prefix must
name a **registered** tenant, the secret must be plain alphanumeric. Either
failure yields one indistinguishable 401 — routing is not a tenant oracle.

### Why a forged prefix fails safely

`tenant_b.<my-own-secret>` routes the lookup **into B's database**, where that
secret does not exist → 401. There is no shared session table for it to be
found in. That is the no-shared-surface property WO-039 measured and the Chunk
A decision record refused to trade away, now load-bearing in the hot path.

### Why resolution is not re-run per request

`TenantRouter::build` resolves **every registered tenant through the
strategy** once, at boot, and stores the result. Membership of the router is
therefore identical to membership of the registry: a prefix that finds a
context has been through `TenancyStrategy::resolve`, **by construction** —
and `build` additionally refuses a context whose broker is scoped to a
different tenant than the key it is filed under. Re-resolving per request
would add hot-path work to re-derive a deterministic answer, and would not
make the answer any more trusted.

## What changed in the request path

`Rest` no longer holds "the broker". It holds an `Arc<TenantRouter>`, and
`dispatch` and everything under it take a `&TenantContext` — 47 call sites
that used to read an ambient `self.broker`. A single-tenant kernel is now
**a router with one route**, not a special case: `Rest::single(..)` builds a
one-entry router and takes the identical path.

`main` boots **every** tenant on the roster — ADR-008's meta discipline is per
database, so it runs per tenant — and builds a broker, a `MetadataSync`, a
resident worker and its rollup workers for each, all sharing **one** wasmtime
engine (criterion 2). The `roster > 1` refusal is gone; it was Chunk B's
honest placeholder for exactly this.

The request trace's `tenant` label now comes from the **token's** tenant. With
N tenants in one process, "which tenant was this request" is only answerable
per request.

## The exit proof

`one_process_routes_two_tenants_and_no_request_ever_crosses` — **one `Rest`,
one port, one process, one shared `Arc<WasmHooks>`**, two tenants, 8 threads,
**48 HTTP reads through the shipped path**, 0 foreign rows.

The crux: *the same URL serves both tenants, and only the token differs.*
Every assertion checks **whose** row came back, and the run is non-vacuous by
the same construction as Chunk A —

- each tenant's read count must equal `threads × rounds` (the run happened)
- no read may return zero rows (it read something)
- the maximum read must equal `1 + threads × rounds`: its seed row plus **its
  own** writes and nothing else, so a leak that merely *adds* rows fails on
  the count as well as on the titles

Then the two refusals, over the same live surface:

| presented | result |
|---|---|
| `wo040c_rt_b.<tenant A's secret>` | **401**, zero rows |
| `wo040c_not_a_tenant.<secret>` | **401** (identical error — no oracle) |

Chunk A's shared-*engine* test is kept alongside it: it exercises the broker
seam directly, which is a narrower claim than the HTTP one and still worth
holding.

## The inverted test

`a_roster_larger_than_this_binary_can_route_refuses` was Chunk B's enforcement
of the gap. B2 closed the gap, so the test was **inverted rather than
deleted**: `a_multi_tenant_roster_now_boots_every_tenant_on_it` provisions two
databases, boots the real binary on `FRUST_TENANTS=a,b`, and asserts **both**
appear in the boot log. "Served a subset" now fails loudly, which is the
property the old refusal was protecting.

## Numbers

Suite: **299 passed · 0 failed · 6 ignored, 49 result groups, exit 0.**

**Submit floor (release):** submit warm median **4 ms** (gate 25) · hook chain
**0 ms** (gate 30) · realtime tax **0.12 ms** (allowance 2).

**Footprint (`tenantmem`, 1/10/50):** 62.4 / 62.3 / 62.3 MB. Flat — routing
adds no per-tenant state. (Chunk B: 63.7 / 61.2 / 63.1.)

**Throughput** (`loadbench`, release, 10 s per rung, dedicated scratch
data-dir, settle pauses between rungs, dev store restored afterwards):

| concurrency | WO-026 | Chunk A | Chunk B | **Chunk B2 (warm samples)** |
|---|---|---|---|---|
| 10 | 123.6 | 133.8 | 135.9 · 136.7 | **137.1 · 137.5 · 137.8** (p50 ~70 ms) |
| 50 | 120.7 | 135.0 | 135.8 | **141.2 · 139.2** |

0 errors at every rung. **The 124 req/s baseline is held and bettered with
per-request routing in the path.**

## FINDING — one sample nearly produced a false regression

The first c=10 read was **115.8 req/s**, below the 124 baseline, and the
second was 121.7. Read alone that is "routing cost us 7%", and the second
sample looks like confirmation. The third, fourth and fifth were **137.1,
137.5, 137.8** — a spread of 0.7 req/s, with p50 ~70 ms against Chunk B's
71.0. The first two were warm-up on a cold scratch store, which is exactly
what the tight convergence afterwards demonstrates.

**This is the more dangerous shape of instrument failure.** Chunk B's
`3.3 req/s` was *impossible* and forced an investigation; `115.8` is
*plausible*, and a plausible bad number invites you to accept it and write
"small cost, acceptable for the feature". The rule that caught it:

> **One sample is not a measurement.**

The same lens, applied backwards, corrects an earlier claim of mine. **The
c=1 rung is not a usable comparison metric on this box** and should stop being
quoted as one: across chunks it has read 26.5, 26.3, 32.1, 43.2, 34.4, 40.9
and 26.7 req/s. At one client throughput is just 1/mean-latency over ~300
samples, so it is tail-dominated and swings ~60%. Its p50 (22.8–36.1 ms) is
the only stable thing about it. I previously called Chunk A's low c=1 "tail
variance" and Chunk B's high one "confirmation" — both were single samples,
and neither claim was earned. The honest statement is that c=1 throughput on
this hardware carries no signal at this sample size.

## The WO-041 interaction, stated in advance and honoured

Per the order: the throughput leg is measured against a kernel that opens
~1 TCP connection to SurrealDB per query ([[WO-041 Connection Reuse]]), so
ephemeral-port pressure is a **load-generator** failure, not a B2 regression.
Rungs were run with settle pauses and TIME_WAIT recorded before each (900 →
1861 → 3681 → 5219 across the cold pass). No rung stalled this time; the
warm-up effect above is a separate, milder contamination of the same class.

## Files

`kernel/src/router.rs` (new — `TenantContext`, `TenantRouter`,
`subdomain_of`) · `rest.rs` (router + per-request `ctx`, tenant-prefixed
token mint, split-before-any-DB-call) · `main.rs` (N-tenant boot, N workers,
one engine) · `tenancy.rs` (`resolve_all`) ·
`tests/tenant_isolation_concurrent.rs` (the HTTP exit proof) ·
`tests/tenancy_boot_refusal.rs` (roster test inverted) · ~17 `Rest`
construction sites migrated to `Rest::single`

## Related
[[WO-040 Multi-Tenant Routing]] · [[ADR-003 Tenancy Model]] ·
[[2026-07-28 WO-040 chunk A tenant routing decision]] (the (a)/(b)/(d) ruling
this executes) · [[2026-07-29 WO-040 chunk A tenancy seam]] ·
[[2026-07-29 WO-040 chunk B strategy parity]] · [[WO-041 Connection Reuse]] ·
[[2026-07-28 WO-039 multi-db tenancy probe]] (the provenance rule) ·
[[2026-07-26 WO-026 surrealdb write concurrency]]
