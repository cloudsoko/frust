---
tags: [frust, build-log, tenancy, security, decision, milestone-4]
created: 2026-07-28
status: SUPERSEDED IN SCOPE, RULINGS INTACT — WO-040 was rescoped the same day to build the TenancyStrategy seam; the token mechanism below is reconciled under ADR-003 invariant 3 and now belongs to Chunk B. See [[2026-07-29 WO-040 chunk A tenancy seam]].
work-order: "[[WO-040 Multi-Tenant Routing]]"
---

> [!warning] Read this as a **ruling record, not a plan**. The Boss rescoped
> WO-040 on 2026-07-28: Chunk A became the `TenancyStrategy` seam (built and
> guarded — [[2026-07-29 WO-040 chunk A tenancy seam]]), and per-request
> routing moved to Chunk B. The (a)/(b)/(d) reasoning below **stands
> unchanged** and is what Chunk B implements; the reconciliation is that the
> token prefix is the *entry point* carrying a canonical `TenantId`, and
> registry-validated resolution through the strategy is the *mechanism*.

# WO-040 Chunk A — How a Request Finds Its Tenant Database

A security-shaped decision record, captured before the code (WO-008
identity-decision precedent). This is an **implementation under
[[ADR-003 Tenancy Model]]**, not an amendment — the tenancy model is unchanged
(database-per-tenant); this is only *how a request finds its database*.

## The problem the probe did not surface

[[2026-07-28 WO-039 multi-db tenancy probe]] concluded routing was "largely: use
the caller's database from the JWT's `ns`/`db` claim." That is true of
**SurrealDB's native signin**, which is what the probe exercised. The kernel does
not use that path:

- `/login` mints an **opaque random token** and stores the real JWT in a
  `_frust_session` row (`rest.rs:561-571`).
- Caller resolution does `SELECT user, role, jwt FROM _frust_session WHERE
  token = '<opaque>'` (`rest.rs:623`).

Both run against the **fixed `cfg.db`**. So the kernel must know *which database
to query* before it can resolve the caller — and the caller's tenant is only
discoverable *from* the session row it has not found yet. A bootstrapping
problem, invisible to the probe because the probe never used the opaque
indirection.

## Options

| | mechanism | verdict |
|---|---|---|
| **(a)** | **Tenant-prefixed opaque token** — `tenant.<random>`; sessions live in each tenant's own DB; routing is a string split *before* any DB call | **CHOSEN** |
| (b) | A control database holding every tenant's sessions with a tenant column | **Rejected** |
| (c) | Explicit tenant on every request (subdomain/header) | Subsumed by (a) |
| (d) | Use a self-verifying JWT directly, drop the opaque indirection | **Rejected** |

### Why (b) is rejected — it hands back the guarantee the milestone exists to gain

A control DB of all sessions re-introduces the shared cross-tenant surface the
probe just proved **absent**, and the worst possible one: not a data table but
the **auth** table. Compromise it and every tenant's sessions are forgeable.
Database-per-tenant's whole value is that a kernel bug *cannot* leak because
there is no shared surface to leak from; (b) returns that surface as a single
high-value target.

### Why (d) is rejected — it would cost instant revocation

The opaque indirection is not incidental: it is *why* WO-033's admin revoke is
instant (delete the row, bump the generation). A self-contained JWT cannot be
revoked without a blocklist — which is another shared table, i.e. (b) wearing a
different hat. (a) keeps the indirection and simply moves the row into the
tenant's own database.

### Why (a) beats (c)

(c) re-sends the tenant on every request and changes the whole client contract.
(a) needs the hint **only at login** and rides the token thereafter — transparent
to the client from that point on.

## The ruled mechanism (all three steps — the second is the one that is easy to miss)

1. **Login-time tenant hint.** `/login` has no token yet, so it must be told
   which tenant's user table to authenticate against — subdomain
   (`acme.frust.app`) or a tenant field in the login payload. *This is the second
   bootstrapping wall; "route by splitting the token" only covers authenticated
   requests.*
2. **Mint `tenant.<random>`** after authenticating in that tenant's database.
3. **Authenticated requests route by splitting the token** before any DB call;
   the session row and its lookup live in the tenant's own database.

## Security constraints on the build

- **Slug, not id.** The prefix is the tenant slug the customer already knows
  (their subdomain). Never a sequential id — `tenant_47.<random>` leaks both
  "you are customer 47" and the customer count.
- **Spoofing fails safe.** Presenting `tenant_b.<my-own-random>` looks up a token
  that is not in B's `_frust_session` → 401. No cross-tenant read is possible
  because the lookup happens *in B's database* and finds nothing.
- **Isolation assertions check provenance.** Every isolation test asserts *whose*
  row came back, never that a call succeeded or failed. This is the WO-039
  near-miss made a standing rule: a routing bug serving tenant B's data to
  tenant A is the one defect this WO cannot ship, and status codes cannot
  detect it.

## Properties preserved

1. **No shared cross-tenant surface** — WO-039's strongest result, intact.
2. **Instant revocation** — WO-033's mechanism, intact, now per-tenant.

## Status

**Ruled and buildable; code not written.** Paused deliberately at a clean
boundary rather than begin a `rest.rs` rewire that could not be finished *and*
verified in one sitting — the same call as the last pause, for the same reason.
The tree is untouched and green.

Chunk A's exit criteria stand: two tenants in one process, isolation proven by
provenance, `tenantmem.rs` flat, single-tenant 25 ms floor and 124 req/s
unregressed, landing at the correct-but-cache-coupled boundary with the coupling
**named** (Chunk B removes it).

## Related
[[WO-040 Multi-Tenant Routing]] · [[2026-07-28 WO-039 multi-db tenancy probe]] · [[ADR-003 Tenancy Model]] · [[2026-07-28 WO-033 kernel hygiene]] (instant revoke) · [[v2.0 Deployability Gate]]
