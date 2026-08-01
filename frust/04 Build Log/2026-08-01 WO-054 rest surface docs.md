---
tags: [frust, build-log, docs, rest, api, adr-016, milestone-5]
created: 2026-08-01
work-order: "[[WO-054 REST Surface Docs]]"
status: DELIVERED — the REST surface is documented from the code, all 41 examples execute against a live kernel, and the route table is guarded against drift in both directions (watched red both ways). Evolution policy normative. ONE ESCALATION — G1, bad credentials answer 500 — reported and deliberately NOT documented as a promise.
---

# WO-054 — REST surface docs + evolution policy

Home: `frust-kernel/docs/` — [README](file:///D:/Dev/rust/frust-kernel/docs/README.md) ·
`rest-api.md` · `evolution-policy.md` · `byo-quickstart.md` · `gaps.md`.
Harness: `frust-e2e/docs.spec.mjs` (`pnpm docs`).

## ESCALATION — G1: bad credentials answer `500`, and say "http status: 404"

The WO's escalation clause: *a route whose behaviour can't be documented
honestly without fixing it is a surface bug, and docs don't launder those.*
This is that route, and it is the authentication path.

Every failed `/login` — wrong password, unknown user, missing `pass` — returns

```
500  {"error":{"kind":"db","detail":"signin transport: http status: 404"}}
```

**It contradicts its own source.** `rest.rs`: `db.signin(user, pass)?; // bad creds -> PermissionDenied`.
The intended answer is 403.

**Why documenting it would launder it.** The current behaviour is a false
statement about the system's health: a client cannot distinguish "your password
is wrong" from "the kernel's database is broken"; every failed login reads as a
server fault to anything watching 5xx, on the highest-traffic error path in the
product; and it leaks an internal transport detail to an unauthenticated
caller.

**Mechanism.** SurrealDB's `/signin` answers **404** for bad credentials.
`Db::signin_inner` maps any non-success to `BrokerError::Db`; `status_for` maps
`Db` to 500. Nothing distinguishes "credentials rejected" from "transport
broke".

**Disposition, pending a ruling.** `rest-api.md` promises **no** status for
this case and says why. The harness asserts only the security property — *bad
credentials do not return a token* — and prints the observed status as a note,
so the defect is visible without being pinned as a contract. **The fix is one
match arm** (map 401/403/404 from the signin endpoint to `PermissionDenied`),
and it needs a test on *both* sides or it trades a false "server broken" for a
false "your password is wrong" when the database really is unreachable.

## Criterion 1 — inventory from the code

Extracted mechanically from `rest.rs` rather than read: **31 routes** (30 match
arms + `/metrics`, which is answered before routing), **15 manager-tier**, 3
auth tiers, the full `status_for` error-kind table, and the conventions a
consumer cannot guess (decimal-string money, `<TenantId>.<random>` token
discipline, dataless realtime ticks, the docstatus lattice).

Documenting found the warts the WO predicted — eight, in `gaps.md`, each with
what it costs a consumer and what a fix would take. Beyond G1 the notable ones:
**the kernel does not route on HTTP method at all** (G2 — `GET /write/…` works),
`/write` answers `{"created": …}` for updates (G3), and an `op` field is
accepted and silently discarded (G4 — which this author sent for an entire
session before reading the code).

## Criterion 2 — every example executes, and the table cannot drift

`frust-e2e/docs.spec.mjs`, **41 checks, 0 failures** against a live
`frust serve`. It asserts documented response *shapes*, not just 200s — money
comes back a string, a new document is docstatus 0, a partial update leaves
untouched fields alone, an app's refusal is 422 naming its app, a role-denied
transition carries a `FRUST:E_WORKFLOW:*` code, a logout kills the token
immediately.

**The anti-rot device is the interesting half:** the harness parses the route
table out of `rest-api.md` *and* out of `rest.rs` and compares the sets, so a
route added without documentation fails the run and a documented route that no
longer exists fails the run.

**Both directions watched red, then green** — and the first was not staged: the
guard's first run failed with

```
undocumented: /app/{}/disable, /app/{}/enable, /subscribe/{}, /events/{}, /unsubscribe/{}
```

— five routes my prose described but the table did not expose. The other
direction was proven by planting `/planted/{fake}` in the docs (`not in source:
/planted/{}`) and removing it. A guard whose failure nobody has seen is
decorative; this one has now failed both ways for real reasons.

## Criteria 3-4 — the policy, and the BYO client

`evolution-policy.md` is normative: only what is documented is promised
(`detail` prose explicitly is not); additive-only within a major, with the
non-shrink list spelled out; breaking changes are versioned majors with a
deprecation window; clients **must** tolerate additive change or they are not
covered.

Its section 4 borrows WO-053's measured result: *the unit of additivity is the
thing that is served.* ADR-006 edge-1 proved that growing an existing interface
breaks every consumer while a new one beside it does not — the REST analogue is
**add a route, do not widen a route's contract**, and the test is "can a client
written against yesterday's document still be correct?"

`byo-quickstart.md` drives login → schema discovery → read → write → workflow →
realtime → logout in plain `curl`, with no Desk and no SDK, and every step is in
the harness. Two points it makes that a consumer would otherwise learn the hard
way: `role` from `/login` is for deciding what to *render*, never what to
*allow* (the kernel enforces); and `Submit` leaves docstatus 0 while `Approve`
crosses to 1, so workflow state and the lattice are two different things.

## Criterion 6 — the riders, one of which corrected its own premise

**(a) The registry round-trip defect — premise corrected by measurement.**
WO-053 reported that the registry holds a manifest its own `/app/update` door
rejects, and inferred install was laxer than update. **I tested both entry
paths and it is not:** a `\uXXXX` lone surrogate is refused by the JSON parser
(`bad json body: lone leading surrogate`), and raw invalid UTF-8 never reaches
the parser. The asymmetry does not exist; the corrupt dev-store row predates the
current door or arrived out of band. The bad row was real, its stated cause was
wrong, and `gaps.md` records the correction.

What the probe *did* find is a real (small) defect, and that is what got fixed:
`read_to_string` fails on invalid UTF-8 and leaves the buffer **empty**, and the
discarded error turned "your bytes are not UTF-8" into `missing field
'manifest_version'` — sending an operator to inspect their manifest's shape,
which is not the problem. Now `400 request body is not valid UTF-8`, asserted in
the harness.

**(b) The stale `wasm-spike/wit` fork is deleted** — and deleting it exposed
something worth stating: it was the only thing keeping three spike binaries
(`spike-host`, `coldbench`, `hookrunner`) compiling. They bind the pre-WO-005
toy world (`record doc {id, status, total: f64}`), so they have been **unable to
load any shipped artifact for many milestones** — they compiled, they could not
have run. Porting them is a rewrite of ~15 call sites across 492 lines, with the
`total: f64` sites needing WO-016 money care rather than a rename; doing that
under a rider would be scope creep. Recorded in `wasm-spike/host/RETIRED.md`
with the exact port recipe and the instruction to bind the canonical WIT if they
are ever wanted back — rather than leaving a silently broken crate.

## Criterion 5 — home, and suites

Docs live in-repo at `frust-kernel/docs/` where a BYO consumer finds them; this
log is the vault's link and carries the gaps list.

Full suite re-run after the UTF-8 change: **58 binaries, 356 passed, 0 failed**
(jwt). The docs harness is registered as `pnpm docs` alongside the other e2e
suites, so it re-runs with them.

## What is NOT delivered, stated

- **G1 is not fixed** — reported for ruling, per the WO's escalation clause and
  its documentation-not-redesign boundary.
- **G2-G8 are not fixed** — findings, as the WO directed.
- The docs cover the **kernel's** surface. The Desk's HTML surface and the
  bundle/manifest schema are explicitly out of the policy's scope and say so.
- The three retired spike binaries are not ported.

## Related
[[WO-054 REST Surface Docs]] · [[ADR-016 Frontend Posture]] (the priced
obligation, first installment) · [[ADR-006 Plugin Capability Surface]] (edge-1,
whose measured result the policy borrows) · [[2026-08-01 WO-053 hook vocabulary]]
