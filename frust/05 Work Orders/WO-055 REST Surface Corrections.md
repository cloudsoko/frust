---
tags: [frust, work-order, rest, api, kernel, milestone-5]
status: DELIVERED (2026-08-01) — G1/G3/G4/G5 fixed, G2/G6 deferred as ruled. **Escalation checked FIRST and did not fire — but only just:** SurrealDB answers 404 for a wrong password AND for a missing database, so the obvious 404→401 fix would have told an operator their password was wrong when their store had vanished. The distinction is drawable from the BODY, not the status; the classifier recognises the one rejection marker and defaults everything else to a server fault. Side effect worth keeping: ADR-013's keyguard proved fail-closed for real (an early cut fed it a parse error and boot REFUSED). See [[2026-08-01 WO-055 rest surface corrections]].
created: 2026-08-01
---

# WO-055: REST Surface Corrections

## Why

WO-054 documented the surface and — as documenting always does — found warts. Six named in `gaps.md`. This WO fixes the ones that are *bugs* (a client is misled or an operator is), leaves the cosmetic ones to the evolution policy, and updates the docs + harness to match. Every fix additive-only unless it can't be, per ADR-016's just-ratified policy.

## Rulings (what to build)

1. **G1 — bad credentials answer 500 saying "http status: 404" — FIX (the escalation).** A failed login must be a typed **401** (`FRUST:E_AUTH_REJECTED`, no internal transport detail), never a 500. Today `signin_inner` maps *any* non-success to `Db`→500, so a real DB outage and a wrong password are indistinguishable — a false "server broken" on every typo, and an internal `signin transport: 404` leaked to an unauthenticated caller. **The fix needs a test on BOTH sides** (WO-054 named this): wrong-credentials → 401 (SurrealDB's 404-for-bad-creds mapped), *and* a genuine signin-transport failure (relay down) still surfaces as 500/`Db` — or the fix trades a false "server broken" for a false "wrong password" when the DB really is down. Distinguish by the actual SurrealDB response, not by assuming.
2. **G4 — silently-ignored `op` field — FIX.** An unknown top-level key on `/write` is **refused** (`FRUST:E_UNKNOWN_FIELD`), not discarded. A client sending `{"op":"create", "record": …}` believing it forces a create, and silently getting an update, is the silent-wrong class on the write path. Refuse the unknown key; the create-vs-update discriminant stays `record` presence, now documented as the *only* discriminant. (Breaking for anyone sending `op` — but nobody is, since it never did anything, and the whole point is that it lied.)
3. **G3 — `/write` says `created` on updates — FIX additive.** Add `record` (the id) to the response alongside `created`; keep `created` as a boolean that is now *correct* (true on create, false on update) rather than always-present-and-misleading. If `created` currently carries the id not a bool, the additive move is a new `action: "created"|"updated"` field, `created` deprecated in the docs. Pick the shape that's additive; state it.
4. **G5 — `/health` is readiness-by-accident — FIX additive.** Add `/ready` reporting explicit boot state (booting/ready), leave `/health` as liveness. This is the ~25s-accepting-boot operational caveat (WO-019) given an honest endpoint so a health check can stop killing kernels mid-boot. Purely additive.

## Deferred (ruled, not fixed here)

- **G2 — no HTTP-method routing.** Real for anything in front of the kernel (a proxy may cache a `GET /write`), but the fix touches every route arm and needs a breaking-vs-additive ruling of its own. **Its own WO** when a deployment puts a cache/proxy in front — named in `gaps.md`, not urgent for a BYO client that controls its own verbs. Do NOT balloon this WO into it.
- **G6 — prose `detail` strings carrying internals.** The evolution policy already declares `detail` unpromised, which is the right containment. Scrubbing internal phrasing from every `detail` is polish, not a bug — leave it, except where a fix above already removes one (G1's transport detail goes).

## Exit criteria

1. G1/G3/G4/G5 fixed; each with a test, G1 with **both-sides** coverage (bad-creds 401 AND real-transport-failure still 500).
2. `docs/rest-api.md` updated to the new truth; `gaps.md` entries moved to a "fixed in WO-055" section (kept, not deleted — the record of what documenting found); the `docs.spec.mjs` harness updated so its examples assert the new shapes and stay green.
3. Both auth modes; fresh-store gates (write/auth path touched); scratch dropped.
4. Live through `frust serve` for G1 (a real wrong-password login over HTTP returns 401, browser or curl) — tested-seam≠wired applies to bug fixes too.

## Escalation

- If G1's both-sides distinction can't be drawn from SurrealDB's actual responses (e.g. it returns the same status for bad-creds and a half-down relay), stop and report — mapping them to the same code would be honest, and that's a finding, not a workaround.
