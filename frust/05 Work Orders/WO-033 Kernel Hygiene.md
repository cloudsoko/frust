---
tags: [frust, work-order, kernel, hygiene, security, v1.1]
status: COMPLETE (2026-07-28) - Item 1: POST /revoke/{user} manager-tier, reuses logout's delete+generation-bump; proven live (22 sessions revoked, next request 401 with no TTL wait, manager's own session survives, clerk gets 403). Item 2: Span::err_at + Db::sql_with_auth_quiet at the ONE keyguard call site - log level only, Result untouched; healthy boot 1 -> 0 lvl:error lines measured before/after, and ADR-013's three-way proof + canary stay green (4/4). See [[2026-07-28 WO-033 kernel hygiene]].
created: 2026-07-28
---

# WO-033: Kernel Hygiene Bundle (Two Loose Ends Before the v2.0 Gate)

> [!info] PM work order — the last v1.1 kernel work before the v2.0 gate. Two small, coherent kernel items (both kernel-side, both closing a named loose end from a prior WO). Bundled because each is too small for its own WO and they share the kernel + full-suite hygiene run.

## Item 1 — Admin revoke-session endpoint (from WO-026)

WO-026 ruled a 5 s TTL backstop for out-of-band session revocation, with the cleaner shape queued: **a first-class kernel revoke endpoint that bumps the session-cache generation → instant revocation**, demoting the TTL to a pure safety-net for truly-out-of-band DB deletion. Frappe has admin session revocation; Frust should.

- Manager-tier endpoint that revokes a session (or all of a user's sessions) — deletes the session record(s) AND bumps the generation so the cache drops immediately, no 5 s wait.
- Prove: a revoked session's very next request is refused (no TTL wait), other sessions survive (coarse-generation-bump re-reads, not collateral loss — the WO-026 property).
- The 5 s TTL stays as the backstop for direct-DB deletion that bypasses even this endpoint.

## Item 2 — Healthy-boot error-log fix (from WO-029)

WO-027's keyguard self-forge test is a *deliberately-failing* call (mint a token with the published constant, confirm the store rejects it). `db.rs`'s "failures ALWAYS emit" rule logs it `lvl:error / E_DB` on **every healthy boot** — so a healthy boot cries `error`, undermining "errors are real" (WO-010 observability discipline).

- The keyguard's intentional-probe failure must NOT log at error level (it's an expected negative, not a failure). Suppress/downgrade at the keyguard call site, not by weakening `db.rs`'s general rule.
- Prove: a clean healthy boot emits **zero** `lvl:error` lines; the guard still refuses a genuinely-restored store loudly (the WO-027 three-way proof stays green — a real compromised key still errors).

## Exit Criteria

1. Both items done and independently proven (endpoint refuses instantly + zero-error healthy boot).
2. **The keyguard's real refusal still fires** — item 2 must not blind the guard; ADR-013's three-way proof (refuse-restored / boot-healthy / boot-then-401) stays green.
3. Full suite green; both perf gates on a fresh store (item 1 touches the session-cache hot path — confirm no floor regression, WO-026's 124 req/s posture holds).
4. Update the v1.1 backlog: both items were named loose ends; strike them.

## Escalations

Standard rules + full hygiene set. If suppressing the keyguard's expected-fail log cleanly requires threading an "expected failure" signal through `db.rs`, report the shape — a boolean at one call site is fine; a general "expected failure" concept in the transport is a design change worth a sentence.

**Related:** [[Frust Hub]] · [[2026-07-26 WO-026 surrealdb write concurrency]] (TTL ruling) · [[ADR-013 Signing-Key Integrity at Boot]] (the keyguard) · [[2026-07-25 WO-010 Observability]] (errors-are-real) · v2.0 gate next
