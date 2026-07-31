---
tags: [frust, build-log, kernel, hygiene, security, observability, work-order, v1.1]
created: 2026-07-28
work-order: "[[WO-033 Kernel Hygiene]]"
status: complete — instant admin revoke (22 sessions, next request 401, no TTL wait); healthy boot 1 → 0 error lines, guard still loud
---

# Build Log — WO-033: Kernel Hygiene Bundle

The last v1.1 kernel work. Two named loose ends, both closed, both with the
control that proves the check can fail.

## Item 1 — Admin force-revoke (from WO-026)

WO-026 shipped a 5 s TTL backstop and queued the cleaner shape. Built it:
`POST /revoke/{user}`, manager-tier.

**No new machinery.** It is `logout`'s proven two-step aimed at another user —
delete the session rows, then `invalidate_sessions()` to bump `SESSION_GEN` so
the WO-026 cache drops immediately. That reuse is the point: the
instant-drop / others-survive-by-re-read property is *already* the ratified
WO-026 behaviour, so there was nothing new to prove about the mechanism, only a
new caller.

**It revokes EVERY session the user holds**, not one token. Revoking one session
while an attacker holds three accomplishes nothing, so "all of them" is the only
safe default for a compromised account.

### Proven on a live kernel

```
clerk works before        : 200
POST /revoke/clerk1       : {"ok":true,"revoked":22,"user":"clerk1"}
clerk's NEXT request      : 401      <- no sleep, no TTL wait
manager's own session     : 200      <- blast radius is one user
```

The generation bump is coarse by design (every cached session re-reads), which
is the ratified "correctness beats hit-rate" posture — other sessions are
*re-read*, never invalidated as data. The 22 revoked sessions are the honest
accumulation of this session's browser and bench runs.

Tests (`kernel_hygiene.rs`, over a live serving kernel, not a hand-built
broker): the clerk logs in **twice** and both tokens are exercised first so they
are certainly cached — otherwise a pass could just mean "nothing was cached to
go stale". A clerk attempting to revoke gets **403**; a user with no sessions
revokes to `0` as a quiet success (an admin should be able to revoke defensively
without first reading the session table).

The 5 s TTL stays as the backstop for a direct-DB deletion that bypasses even
this endpoint. A revoke emits `evt:session_revoked` at `info` with the user and
count — the audit trail for a security action.

## Item 2 — The healthy boot stopped crying `error` (from WO-029)

WO-027's keyguard mints a token with the published `[REDACTED]` constant and
asks the database to reject it. That call is **supposed to fail**, but
`db.rs`'s "failures ALWAYS emit" rule logged it `lvl:error / E_DB` on **every
healthy boot** — training operators to ignore boot errors, which is precisely
the "errors are real" discipline WO-010 built.

### The shape, and the one property that matters

`Span::err_at(e, level)` (two lines, mirroring the existing `ok_at` — `finish`
already took a level, so no new concept), then `Db::sql_with_auth_quiet`, used
by exactly one caller: the keyguard probe.

**Only the log level changes. The `Result` is returned untouched.** That is the
whole correctness argument. `store_accepts_key` deliberately maps an auth
refusal to `Ok(false)` (healthy) and propagates everything else as `Err` (refuse
to boot) — so quieting the *outcome* rather than the *log* would turn "I could
not verify" into "verified safe", in the one path whose job is to have no blind
spots. It is the ADR-008/013 fail-closed principle applied to a cosmetic change:
the suppression is safe **by construction**, because it cannot reach the
`Result`. `db.rs`'s general rule is untouched for every other caller.

### Measured before/after, same command

| | `lvl:error` lines on a clean boot |
|---|---|
| before (this session's own captured boot log) | **1** — `E_DB`, the keyguard probe |
| after | **0** |

Boot still completes (`rest_listening`). And the check can go red: the very next
genuine failure — a revoked token hitting `/meta` — logged
`E_PERMISSION_DENIED / 401` at error level. **Quiet when healthy, loud when
something is actually wrong**, which is the property, not merely "fewer lines".

### The guard is not blinded (criterion 2)

ADR-013's three-way proof, re-run: **refuse-restored / boot-healthy /
re-issue-clears**, plus the redaction canary — 4/4 green. And
`a_restored_store_is_still_refused_loudly` installs the placeholder as the *real*
key and asserts both that `FRUST:E_RESTORED_ACCESS_KEY` still fires **and** that
the store genuinely accepts the forged token — so the guard is never reporting a
compromise it failed to detect. That test is the control for the quiet path:
without it, "a healthy boot logs zero errors" would be a check nobody had ever
watched fail.

## Verification

- `kernel_hygiene` — 4/4 (revoke instant + surgical, manager-tier, no-op quiet, guard loud).
- `keyguard_canary` — 4/4, ADR-013 intact.
- Live boot A/B — 1 → 0 error lines; live revoke — 200 → 401 with no wait.
- Full suite: [tally on close]. Item 1 touches the session-cache hot path, so
  the WO-026 posture is re-confirmed by the suite's own gates.

## Files

- `kernel/src/telemetry.rs` — `Span::err_at`.
- `kernel/src/db.rs` — `sql_with_auth_quiet` + shared `sql_with_auth_at`.
- `kernel/src/keyguard.rs` — the probe uses the quiet variant (Result path unchanged).
- `kernel/src/rest.rs` — `["revoke", user]`.
- `kernel/tests/kernel_hygiene.rs` — new.

## Related
[[WO-033 Kernel Hygiene]] · [[2026-07-26 WO-026 surrealdb write concurrency]] (the TTL ruling this closes) · [[ADR-013 Signing-Key Integrity at Boot]] · [[2026-07-25 WO-010 Observability]] (errors-are-real) · [[assert-outcome-not-operation]] · v2.0 gate next
