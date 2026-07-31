---
tags: [frust, work-order, security, identity]
status: COMPLETED 2026-07-24 — hole closed at 3 depths (binary-authoritative posture re-asserted every boot / identity_guard EVENT typed-fatal / null-safe clauses); canary pins both directions; 9-touchpoint sweep tabled; floor untouched (22–25 ms). REST role-header item → Desk v1. → [[2026-07-24 WO-008 identity hardening]]
created: 2026-07-24
---

# WO-008: Identity Hardening (the `$auth` Sharp Edge)

> [!info] PM work order — security takes the slot ahead of Desk v1. Results to `04 Build Log/`, live vault path verified first.

## The finding (WO-007, latent, not introduced by it)

`$auth.id` in `DEFAULT`/query contexts resolves only if the user can select their own `app_user` record. Under `app_user PERMISSIONS NONE`: `owner` stamps **NULL silently**, then `owner = $auth.id` row clauses pass via `NONE = NONE` **for every non-manager** — a full row-visibility hole one config drift away. This is the third member of a family: silent-NULL identity (this), the root-`$auth` caveat (module 3), and permission clauses comparing NULLs — the common root is *identity resolution failing quiet*.

## Exit Criteria

1. **`app_user` DDL is kernel-owned** (builder recommendation, ratified): its `DEFINE TABLE`/`PERMISSIONS`/self-select posture ships from the binary like the meta-schema (ADR-008 binary-authoritative discipline — sync one-way up, drift = boot refusal or repair, operator DDL on it has no standing). A tenant cannot config-drift `app_user` into the hole.
2. **NULL-identity is loud everywhere:** a write whose `owner`/`requested_by` stamp resolves NULL for a record-user session **fails typed** (`E_IDENTITY_UNRESOLVED`), never stores NULL silently. Prove with the exact drift scenario from the finding: force `PERMISSIONS NONE`, show the write *refuses* instead of stamping NULL.
3. **`NONE = NONE` can never grant:** permission clauses the compiler emits are null-safe (explicit `owner != NONE AND owner = $auth.id` or equivalent). Prove: a NULL-owner row (seeded via root) is invisible to every record principal, not visible to all.
4. **The EVENT-bypass canary** (WO-007's probe, made permanent): a pinned test asserting EVENT-body writes bypass table permissions — the behavior Tier-1 counters *depend on* — so a version bump that changes it fails CI loudly (same posture as the conflict-string canary).
5. **Family sweep:** grep-level audit of every `$auth` touchpoint in kernel + synced DDL for quiet-NULL paths; each either proven loud or fixed. Table in the log.

## Escalations

Standard rules. If null-safe permission clauses measurably regress the 25 ms floor, report the number before trading security for it (expectation: negligible; the floor gate will catch it anyway).

**Related:** [[Frust Hub]] · [[ADR-008 Data Shape]] · [[2026-07-24 WO-007 aggregates ladder implementation]] · [[SurrealDB]] (v3.2.0 caveats)
