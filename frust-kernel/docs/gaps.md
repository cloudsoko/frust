# Surface gaps — found by documenting it

**Status:** G1, G3, G4, G5 fixed in WO-055 (see the bottom of this file).
G2 and G6 remain open, by ruling.

Documenting a surface finds its warts; this is the list, named rather than
papered over. Nothing here is fixed by WO-054 (its boundary was *document what
is*, not redesign). Each entry says what it is, what it costs a consumer, and
what fixing it would take, so the PM can rule on each independently.

G2 and G6 below are **deferred by ruling**, not oversight: G2's fix touches
every route arm and needs its own breaking-vs-additive decision (its own WO,
when a proxy or cache sits in front of the kernel); G6 is contained already by
the evolution policy, which declares `detail` prose unpromised.

---

## G2 — the kernel does not route on HTTP method

`route()` dispatches on path segments only, so `GET /write/sales_invoice` with
a body works exactly like `POST`. Consequences: no CSRF-relevant distinction
between safe and unsafe methods at the kernel, caches and proxies may treat a
mutating call as cacheable, and `HEAD`/`OPTIONS` are not answered specially.

*Cost to a consumer:* low today (they control their own client), real for
anything sitting in front of the kernel. *Fix:* match on method per route;
mechanical but touches every arm, and needs a ruling on whether to reject
mismatches (breaking) or accept-and-warn (additive).

## G6 — error `detail` strings are prose, and some carry internals

Beyond G1, several `detail` strings surface internal phrasing (`signin
transport: …`, `metadata sync errors: [MigrationError { … }]`). The evolution
policy already declares `detail` unpromised, which is the right containment,
but the leakiest ones are worth tidying.

*Fix:* per-site, no contract change.

## G7 — no pagination metadata on `/read`

`limit`/`start` go in; nothing comes back saying how many rows exist, so a
client cannot render "page 2 of 7" without a second `/aggregate` call.

*Fix:* additive — a `total` key, or a documented count recipe.

## G8 — three near-identical "not found" answers

A disabled app, an unknown app, and an unknown route are deliberately
indistinguishable (a ruled decision — a disabled app should not be probeable).
Worth stating so a consumer does not read one as the other; not a defect.

---

## Also found, already fixed in WO-054

- **A non-UTF-8 request body reported `missing field \`manifest_version\``** —
  `read_to_string` fails on invalid UTF-8 and leaves the buffer empty, and the
  discarded error turned "your bytes are not UTF-8" into a message about the
  shape of a manifest. Now `400` naming the actual fault.

## Corrected while writing these docs

- WO-053 reported the registry "holds a manifest its own update door rejects",
  and inferred that install was laxer than update. **Measured here: it is not.**
  Both entry paths already refuse — a `\uXXXX` lone surrogate is rejected by
  the JSON parser, and raw invalid UTF-8 by the body reader (see the fix above).
  The corrupt row in the dev store predates the current door or arrived out of
  band; the intake asymmetry does not exist. The original finding named a real
  bad row, but its stated cause was wrong.

---

# Fixed in WO-057

## ESC (from the WO-056 dogfood) — a refused CREATE reported success · **FIXED**

Was: `POST /write/{write-closed table}` answered `200
{"action":"created","created":null,"record":null}` and created no row — WO-020's
Finding A alive on the CREATE path, where the guard had been written for UPDATE
only and CREATE fell through the catch-all.

Now: `403 permission-denied` / `E_WRITE_NO_ROWS`, naming the table. The
database's refusal is unchanged — only the sentence the kernel says about it.
Both sides tested (`kernel/tests/refused_create.rs`): the refused create is
typed AND a legitimate create still returns its record, because a fix here could
trade a false success for a false failure.

Found by *using the app*, not by a test — and made visible by WO-055's additive
`action`/`record` keys, since `action:"created"` beside `record:null` is
self-contradicting.

---

# Fixed in WO-055

**Kept, not deleted.** These are the record of what documenting the surface
found — the entries stay so the finding is legible, with what shipped.

## G1 — bad credentials answered `500` · **FIXED**

Was: every failed login returned `500 {"kind":"db","detail":"signin transport: http status: 404"}`,
contradicting the route's own source comment, reporting a server fault on the
busiest error path there is, and leaking an internal transport detail to an
unauthenticated caller.

Now: `401 {"kind":"permission-denied","detail":"FRUST:E_AUTH_REJECTED"}`, with a
wrong password and an unknown user deliberately indistinguishable.

**The fix's real difficulty was the half that isn't the bug.** SurrealDB 3.2.0
answers **404 for a wrong password AND 404 for a database that does not exist**,
so the obvious `404 → 401` mapping would have told an operator their password
was wrong when the tenant's store had vanished. The discriminant is the body:

| case | HTTP | `information` |
|---|---|---|
| wrong password / unknown user | 404 | `No record was returned` |
| database does not exist | 404 | `The database 'x' does not exist` |
| namespace does not exist | 404 | `The namespace 'x' does not exist` |
| store present but broken | 400 | `The record access signin query failed` |

So the classifier recognises the one rejection marker and treats **everything
else as a server fault** — the safe default direction. Both sides are tested
(`kernel/tests/login_errors.rs`), including the recorded-responses table as an
executable case list.

Two things fell out of reading the body at all, both worth having:
- every non-2xx from SurrealDB now reports **SurrealDB's own message** instead
  of `http status: NNN`;
- doing it wrong first proved ADR-013's keyguard is genuinely fail-closed: an
  early cut fed it a parse error instead of a recognisable 401 and **boot
  refused** rather than assuming the store was healthy.

## G3 — `/write` said `created` on updates · **FIXED additive**

`action: "created"|"updated"` and `record: <id>` added. `created` still carries
the row, unchanged and deprecated in the docs — removing it would be breaking,
so the policy's additive path was taken instead.

## G4 — an ignored `op` field · **FIXED**

An unknown top-level key on `/write` is refused with `400 FRUST:E_UNKNOWN_FIELD`
naming the key and the accepted ones. Breaking for anyone sending `op` — nobody
is, because it never did anything, and its doing nothing *silently* was the bug.

## G5 — `/health` was readiness by accident · **FIXED additive**

`GET /ready` added, reporting per-tenant boot facts (meta version, DocType
count, orphan columns). `/health` stays liveness.

Stated honestly: the kernel does not accept connections until boot completes, so
over HTTP `/ready` has never been observed `false` — the ~25 s window is a
refused connection. What it adds is the positive signal, the boot facts, and the
split from liveness. A test asserts it reads `false` in a process that never
booted, so the endpoint is not a constant wearing a probe's name.
