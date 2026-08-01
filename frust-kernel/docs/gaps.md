# Surface gaps — found by documenting it

Documenting a surface finds its warts; this is the list, named rather than
papered over. Nothing here is fixed by WO-054 (its boundary was *document what
is*, not redesign). Each entry says what it is, what it costs a consumer, and
what fixing it would take, so the PM can rule on each independently.

**G1 is an escalation, not a wart** — it is the one item the WO's escalation
clause covers.

---

## G1 — bad credentials answer `500`, and say "http status: 404" · **ESCALATED**

**What happens.** Every failed `/login` — wrong password, unknown user, missing
`pass` — returns:

```
500  {"error":{"kind":"db","detail":"signin transport: http status: 404"}}
```

**Why it is a contradiction rather than a wart.** The route's own source says
otherwise — `rest.rs`: `let jwt = db.signin(user, pass)?; // bad creds -> PermissionDenied`.
The intended answer is `403`. So the surface contradicts its stated contract,
and documenting the current behaviour would enshrine a **false statement about
the system's health**:

- a client cannot distinguish "your password is wrong" from "the kernel's
  database is broken";
- every failed login looks like a server fault to any monitor watching 5xx —
  on the product's highest-traffic error path;
- it leaks an internal transport detail (`http status: 404`) to an
  unauthenticated caller.

That is why `rest-api.md` deliberately promises **no** status here, and why the
harness asserts only the security property (no token is issued) rather than
pinning `500`.

**Mechanism.** SurrealDB's `/signin` answers **404** for bad credentials.
`Db::signin_inner` maps any non-success to `BrokerError::Db`, and `status_for`
maps `Db` to 500. Nothing distinguishes "credentials rejected" from "transport
broke".

**Fix.** Small and local: in `signin_inner`, map a 401/403/404 from the signin
endpoint to `PermissionDenied` and leave everything else as `Db`. One arm, plus
a test that a wrong password is 403 and that a genuinely unreachable database
still surfaces as 500 — the second half matters, or the fix trades a false
"server broken" for a false "your password is wrong".

**Not fixed here** because WO-054's boundary is documentation, and because the
mapping is security-adjacent enough to deserve its own ruling.

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

## G3 — `/write` returns `{"created": …}` for updates too

An update's response key says `created`. Purely cosmetic, but a consumer
reading `created` as "a new record was made" is wrong.

*Fix:* additive — add `record` alongside, keep `created` for a major. Renaming
it is breaking.

## G4 — an ignored `op` field is silently ignored

`{"op":"create"}` is accepted and discarded; create-vs-update comes from the
presence of `record`. A client that believes it is asking for a create, while
also sending `record`, gets an update with no complaint. (Observed first-hand:
this document's author sent `op` for a whole session before reading the code.)

*Fix:* refuse an unknown top-level key, or honour `op` and refuse a
contradiction. Both are breaking for anyone currently sending it.

## G5 — `/health` reports the process, not readiness

`{"ok":true}` is answered by the HTTP layer as soon as it listens, but boot
(meta migration + schema sync) can take ~25 s and REST does not listen until
it finishes — so `/health` is honest today *by accident of ordering*, and
carries no explicit readiness/liveness split. Operators have already been bitten
by health checks that kill a kernel mid-boot (banked in WO-019).

*Fix:* a `/ready` that reports boot state explicitly, additive.

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
