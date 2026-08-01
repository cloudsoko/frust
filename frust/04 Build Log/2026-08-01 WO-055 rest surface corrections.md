---
tags: [frust, build-log, rest, api, kernel, adr-013, milestone-5]
created: 2026-08-01
work-order: "[[WO-055 REST Surface Corrections]]"
status: DELIVERED — G1/G3/G4/G5 fixed, G2/G6 deferred as ruled. G1's both-sides requirement turned out to be the whole difficulty: SurrealDB answers 404 for a wrong password AND for a missing database, so the obvious fix would have traded one false signal for another. Escalation did NOT fire — the distinction is drawable, from the body rather than the status.
---

# WO-055 — REST surface corrections

## G1 — the escalation's question, answered by measurement first

The WO's escalation clause was the right thing to check before writing any
code: *if SurrealDB's responses can't distinguish bad-creds from a half-down
relay, mapping them the same is honest and that's a finding.* So I probed every
failure mode against the running store before touching `signin_inner`:

| case | HTTP | `information` |
|---|---|---|
| correct credentials | 200 | (token) |
| **wrong password** | **404** | `No record was returned` |
| **unknown user** | **404** | `No record was returned` |
| **database does not exist** | **404** | `The database 'x' does not exist` |
| **namespace does not exist** | **404** | `The namespace 'x' does not exist` |
| store present, `app_user` absent | 400 | `The record access signin query failed` |
| access method missing | 400 | request problem |
| malformed body | 401 | Authentication failed |

**The escalation does not fire, but only just.** The status is genuinely
ambiguous — a wrong password and a vanished tenant database are *both 404* — so
the obvious `404 → 401` fix would have told an operator their password was
wrong when their store had disappeared, which is precisely the trade the WO
named. The body draws the line the status cannot.

So the classifier recognises the **one** rejection marker and treats everything
else as a server fault. That default direction is the safe one: "the database
is gone" reported as "your password is wrong" sends the operator to the only
place the problem isn't.

Shipped: `401 {"kind":"permission-denied","detail":"FRUST:E_AUTH_REJECTED"}`,
no transport detail, and a wrong password indistinguishable from an unknown
user (otherwise the endpoint is a user-enumeration oracle).

**Both sides tested** (`kernel/tests/login_errors.rs`):
- wrong password → typed 401, asserted to leak neither `http status` nor `404`;
- unknown user → byte-identical to the above;
- **a database that does not exist → still `Db`**, naming which database — the
  half that makes the fix correct rather than half-right;
- the recorded-responses table above as an executable case list against the
  real classifier, so the table cannot rot into a comment.

Live through `frust serve`: wrong password and unknown user both `HTTP 401
FRUST:E_AUTH_REJECTED`; correct credentials still return a token.

### Two things fell out of reading the body at all

1. **Every non-2xx from SurrealDB now reports SurrealDB's own message** instead
   of `http status: NNN`. `read_json` gained the status check as a single
   chokepoint that all five call sites already funnelled through.
2. **ADR-013's keyguard proved genuinely fail-closed, by accident.** Turning
   off ureq's status-as-error meant error bodies reached the parser — and
   SurrealDB answers a forged token with the plain sentence *"There was a
   problem with authentication"*, not JSON. My first cut demanded JSON, so the
   keyguard got `parse: expected value at line 1 column 1` instead of a
   recognisable 401, did not recognise it, and **boot refused**. Exactly what a
   guard that cannot verify safety should do. Fixed by not requiring JSON of an
   error body — and the status stays *in* the message because that is what the
   keyguard recognises.

## G4 — a silently-ignored key becomes a refusal

`{"op":"create", "record": …}` used to be accepted, `op` discarded, and an
update performed — the caller's stated intent and the outcome disagreeing with
nothing said. Now `400 FRUST:E_UNKNOWN_FIELD` naming the offending key **and**
the accepted ones. The test also asserts the refused request **wrote nothing**
(a refusal that still writes is worse than the silent ignore it replaced) and
that a well-formed write is unaffected.

## G3 — the response says which happened, additively

`action: "created"|"updated"` and `record: <id>` added. `created` still carries
the row, unchanged and **deprecated in the docs** — turning it into a boolean
would break every existing reader, so the evolution policy's additive path was
taken instead and the deprecation stated.

## G5 — readiness said out loud

`GET /ready` reports per-tenant boot facts (meta version, DocType count, orphan
columns), recorded by `boot()` itself on its success path so the flag cannot be
true for a tenant that did not boot. `/health` stays liveness.

**Stated honestly rather than overclaimed:** the kernel does not accept
connections until boot finishes, so over HTTP `/ready` has never been observed
`false` — the ~25 s accepting-boot window (WO-019) shows as a *refused
connection*, and a health check must still budget for it. What the endpoint
adds is the positive signal, the boot facts to assert against, and the split
from liveness. A test asserts it reads `false` in a process that never booted,
so it is not a constant wearing a probe's name.

## Deferred, as ruled

- **G2 (no HTTP-method routing)** — real, untouched. Its fix touches every route
  arm and needs its own breaking-vs-additive decision; its own WO when a proxy
  or cache sits in front of the kernel.
- **G6 (prose `detail` internals)** — contained by the evolution policy, which
  already declares `detail` unpromised. G1's transport leak is gone as a side
  effect; the rest is polish.

## Docs and harness followed

`rest-api.md`: `/ready` documented, `/login`'s 401 now a promise (with the
404-vs-404 caveat stated), `/write` carrying `action`/`record` with `created`
marked deprecated, and the unknown-key refusal. `gaps.md`: G1/G3/G4/G5 moved to
a **"Fixed in WO-055"** section — kept, not deleted, because they are the record
of what documenting the surface found. `docs.spec.mjs`: **47 checks, 0
failures** (was 41), now asserting the 401, the no-leak property, the
enumeration-oracle property, `/ready`'s facts, `action` on both paths, and the
unknown-key refusal. The route drift-guard stayed green, which is how I know
`/ready` reached both the docs and the code.

## Verification

- `kernel/tests/login_errors.rs` 3/3, `kernel/tests/surface_corrections.rs` 3/3.
- `docs.spec.mjs` **47/0** against a live `frust serve`, route drift-guard green
  (which is how I know `/ready` reached both the docs and the code).
- **Full suite, both auth modes: 60 binaries / 362 passed / 0 failed each.**
- **Fresh-store gates, both modes**, dedicated scratch data dir, 3 samples each:

  | | hook chain | submit | realtime tax |
  |---|---|---|---|
  | jwt | 0 ms (gate 30) | 3 / 3 / 4 ms (gate 25) | 0.15 ms (allowance 2) |
  | basic | 0 ms | 4 / 4 / 4 ms | 0.30 ms |

  The read path of *every* DB call changed (status handling moved into
  `read_json`), so these re-ran rather than being assumed unaffected. No
  movement claimed against WO-054's 2 ms — different scratch store, and
  machine-sensitivity is the standing caveat; what the gates say is that the
  change did not move the floor against its budget.
- Scratch store and the four `wo055_*` scratch databases dropped; dev stack
  restarted and `/ready` answering with the real boot facts.

## Related
[[WO-055 REST Surface Corrections]] · [[2026-08-01 WO-054 rest surface docs]] ·
[[ADR-013 Signing Key Integrity]] (fail-closed, re-proven by accident) ·
[[ADR-016 Frontend Posture]] · [[SurrealDB]] (the signin response table)
