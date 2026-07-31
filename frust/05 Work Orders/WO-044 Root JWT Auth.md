---
tags: [frust, work-order, performance, security, surrealdb, milestone-4]
status: COMPLETE (2026-07-31) — all 4 criteria met, correctness + ADR-013 gated BEFORE any number (the WO-026 rule). **THE 124 req/s CEILING WAS AN ARGON2 CEILING.** Paired saturation sweep, quiet machine, kernel restarted per arm: Basic saturates **~117 req/s** (= the historical 124), JWT saturates **~555 req/s** — 7.3× at c=2, 4.7× at c=8; p50 21.0→3.0 ms at c=1. Primary A/B on a DEDICATED SCRATCH STORE (own data dir; the example refuses a :8899 endpoint outright): **16.53 ms → 297 µs per root query, 55.6×**, 5 samples/arm, spreads 1.27×/1.17× (converged). WO-043's notification floor: rule overhead **48.0 ms → 4.28 ms**. Submit floor does NOT move (2 ms both modes) — reported as the negative result it is, because WO-026 had already taken root off that path. Live kernel: **614 root calls on 1 signin**. ADR-013 INTACT, and the design turns on it: the re-signin retry is scoped to root-only calls BY DESIGN — in the shared `sql_with_auth` it would have retried the keyguard's *deliberately forged* probe as root, succeeded, and reported EVERY HEALTHY STORE COMPROMISED (a boot-refusing false positive on every deployment); a test asserts a forged token is still 401'd while the kernel can mint a working one. ADR-013's subject (the record-access key) and this change (root auth) are disjoint — verified by grep, the ADR makes no root/credential/bearer claim in 100 lines. Posture stated: a cached root JWT is a bearer secret that EXPIRES in 1 h, strictly narrower than the `root:root` password the kernel already holds in memory and sent on every request. `FRUST_ROOT_AUTH=jwt|basic` escape hatch, unknown value refuses boot; `/signin` unavailable → Basic **loudly** (error + gauge). FOUR FINDINGS: (1) a write costs **~3 root queries** (3.05 and 2.77 by two independent methods) against exactly 1 session call — that IS the old ceiling; WHICH three is open follow-up, explicitly not claimed; (2) an **idle** kernel issued 19 root queries/s = ~314 ms of argon2/s, about a third of a core, permanently; (3) `frust_root_auth` published NO series at all in basic mode (mine, fixed — an absent series is not a reading); (4) `kernel_hygiene::revoke_kills_...` is a **pre-existing parallel flake**, proven not-mine by A/B (jwt 2/5 fail, basic 3/5 fail, serial 0/5) — diagnosed, deliberately NOT fixed (off-order). FIVE INSTRUMENT FAILURES, all mine, incl. predicting the write path had no root queries (wrong — caught by measurement, not by reasoning), a probe that reported `/health` costing 4 root calls because it was measuring the background thread, and a `timeout|grep` harness that nearly made me record a nonexistent concurrency hang. Zero regression: full kernel sweep green (only the pre-existing flake), perf_gates 3/3, workflow 18/18, SSE 8/8, mail 15/15. See [[2026-07-31 WO-044 root jwt auth]].

Prior status — ACTIVE (2026-07-30) — escalated from WO-043's finding. The kernel authenticates every `sql_root` call as `root:root` Basic, and SurrealDB argon2-verifies the password ON EVERY REQUEST → 16.5 ms/call vs 0.79 ms for a cached session Bearer (21×), same query same process. Pre-existing + kernel-wide; WO-043 was merely the first feature to put a root query on the per-request write path. Fix PROVEN: root JWT via `POST /signin` (argon2 runs once at signin), 200 µs server-side. Touches ADR-013's keyguard (its self-forge probe drives the root auth path) → its own order, not slipped into an email WO.
created: 2026-07-30
---

# WO-044: Root JWT for `sql_root` — kill the per-request argon2 tax

> [!info] PM work order — escalated from [[2026-07-30 WO-043 email batteries]] (finding 1). A performance fix with a **security surface**: it changes how the kernel authenticates as root, which is exactly what [[ADR-013 Signing-Key Integrity at Boot]] guards. Correctness and ADR-013 integrity gate any throughput claim. Governing: [[ADR-013 Signing-Key Integrity at Boot]] · [[2026-07-26 WO-026 surrealdb write concurrency]] (the same "measure the dimension you claim" finding, a different place) · WO-024/025 (no hot-path lock) · [[2026-07-28 WO-033 revoke endpoint]] (the generation/refresh pattern this rhymes with).

## The finding (WO-043, measured)

Same query, same process, isolated:

| auth path | median |
|---|---|
| `sql_root` — Basic `root:root` | **16.56 ms** |
| `sql_as` — cached session Bearer (JWT) | **0.79 ms** |

SurrealDB **argon2-verifies the root password on every request**. So every kernel query issued as root pays ~16 ms of KDF cost it does not need to. This is pre-existing and kernel-wide — metadata reads (partly cached by WO-026), job claims, rollup drains, DDL, boot queries — and it is WO-026's lesson again: the connection/round-trip dimension was measured, the *auth-cost* dimension was not. WO-043 exposed it by putting a root query (notification-rule load) on the write path (floor 3→51 ms).

**Fix, proven:** `POST /signin` with root creds returns a root JWT that `/sql` accepts — argon2 runs **once at signin**, not per request — measured at **200 µs server-side**. Mint once, cache, reuse for `sql_root`.

## Exit Criteria

1. **Correctness first (gate everything on it — WO-026 rule).** The root-JWT path returns **identical results** to Basic-root for every `sql_root` consumer (metadata, job claim, rollup drain, DDL, boot). Token refresh works: the cache refreshes before the `DEFINE ACCESS` token TTL, and a `401` from an expired token triggers **re-signin + retry once** (the conflict-retry pattern), never a hard failure or a silent skip. Tests land **before** any throughput number.
2. **ADR-013 integrity is the security spine — prove it still holds.** [[ADR-013 Signing-Key Integrity at Boot]]'s self-forge probe drives the root auth path; the keyguard three-way proof + `keyguard_canary` must stay green, the **fail-closed boot guard must still fire**, and a forged/restored/redacted-key root token must still be **rejected**. The change must open **no** path ADR-013 closes. State the posture explicitly: a cached root JWT is a bearer secret that **expires** — no worse than the `root:root` creds the kernel already holds in memory, and arguably better (revocable/expiring). If any of this can't be preserved, that's the escalation, not a quiet trade.
3. **The win, measured honestly.** ~16 ms should come off every `sql_root` call. Show it: WO-043's notification write floor (51 ms) drops toward the no-root floor; re-measure the throughput/submit numbers (does it move past 124 req/s? does the submit floor drop from 3–5 ms?). Fresh store, dedicated scratch data-dir, ≥3 samples, converge before comparing. Report the delta; don't credit it to anything but this.
4. **No hot-path cost reintroduced.** The JWT is minted once (boot or lazy-first-use), cached, refreshed on a schedule/expiry — **never argon2 on the request path, never a lock on the request path** (WO-024/025). Resolve the cached token the way `Db` resolves its agent: once, off the hot path.

## Boundaries

- **Root AUTH path only** — not the query model. Do NOT touch the WO-026 cache logic or the session-Bearer path (user queries, already 0.79 ms). This changes *how root authenticates*, nothing about *what* it queries.
- **No new dependency** — the kernel already speaks `/signin` and JWT (session auth).
- **ADR-013 is a security boundary** — any change to its guarantees is an amendment, not an implementation detail. The keyguard's canary is load-bearing.
- Root JWT TTL/rotation reuses the existing session generation/refresh machinery where it fits ([[2026-07-28 WO-033 revoke endpoint]] pattern) rather than a bespoke timer.

## Escalation

If a cached root JWT cannot be introduced without weakening ADR-013 (e.g. the self-forge probe can no longer distinguish a healthy key, or the boot guard can be bypassed), **STOP and report the exact conflict** — it becomes an ADR-013 amendment conversation, ruled before any code ships. A 16 ms win is not worth a millimetre of the signing-key integrity ADR-013 exists to hold.

**Related:** [[Frust Hub]] · [[2026-07-30 WO-043 email batteries]] (surfaced it) · [[ADR-013 Signing-Key Integrity at Boot]] · [[2026-07-26 WO-026 surrealdb write concurrency]] · [[2026-07-28 WO-033 revoke endpoint]] · [[v2.0 Deployability Gate]]
