---
tags: [frust, build-log, performance, kernel, milestone-4]
created: 2026-07-31
work-order: "[[WO-048 Server-Script Cache]]"
status: COMPLETE — **the property is achieved: ZERO root round trips on the steady-state write path** (trace-attributed, 20/20 writes, was 1/1). Correctness gated first: the live-mutability control was shown RED with invalidation deliberately broken, then restored. Controlled A/B on a fresh database: **jwt 334.8 → 338.9 req/s — no meaningful change, exactly as the WO predicted** (the path is DB-bound elsewhere); **basic 70.0 → 324.9 req/s, 4.6×, p50 107.4 → 24.0 ms** — where the 16.5 ms root call genuinely hurt. Corroboration: with zero root calls per write, basic (324.9) and jwt (338.9) converge, because the root-auth mode stops mattering for writes. TWO CORRECTIONS TO MY OWN WO-047 RECORD: the `/doctype/{name}/script` route handles **client_script, not server_script**, and it **did not bump at all** — so "the invalidation site already bumps" was wrong twice; there is no server-script save route, server scripts arrive via `POST /doctype` and app install/update, both of which bump. The missing bump was added. The live-mutability test was editing the DB **out of band**, which the cache correctly does not see — rewritten to drive the real path, with the out-of-band boundary pinned by its own new test rather than lost.
---

# WO-048 — Server-script cache: zero root calls on the write path

## What shipped

A generation-invalidated cache of the script text, per `(tenant, doctype)`, in
`hooks.rs`'s `ScriptSource`. It rides the **existing meta generation** — a server
script is a field on the `doctype` record, so every site that already
invalidates DocType metadata invalidates this too, and **no new bump site was
introduced**. That is the property that keeps ADR-007's live-mutability true:
this cache and the doctype cache cannot drift apart, because they are the same
counter.

`None` is cached as deliberately as `Some`: the scriptless doctype is the common
case and it was paying full price, because the query is how you find out there
is no script.

The generation handle is resolved once at construction, never on the write path
— the rule `Db` already follows for its agent (WO-041) and its root credential
(WO-044).

## Criterion 2 — correctness first, and it bit immediately

**The live-mutability test failed the moment the cache landed.** That is the
correctness gate working, and the diagnosis matters more than the fix:

`editing_a_server_script_takes_effect_without_a_restart` revised the script with
a direct `sql_root("UPDATE doctype SET server_script = …")` — it never went
through a kernel door, so it never bumped the generation. That worked only
because nothing cached the script. With the cache, a direct DB write is an
**out-of-band edit**, which is the standing caveat the DocType meta cache has
carried since WO-026.

So the question was not "is the cache wrong" but "what does the ratified
property actually claim". ADR-007's claim is about the **delivered** path: an
operator saves a script → the next write runs it. The test now revises through
an app update — the way a server script is actually revised — and stays green.

**The narrowing is stated, not hidden.** A new test,
`an_out_of_band_script_edit_is_not_seen_until_the_generation_bumps`, pins the
boundary explicitly: a direct table edit is *not* picked up, and the remedy (any
kernel-mediated metadata change) republishes the truth. What was accidental
coverage is now a stated contract that fails loudly if someone widens it.

**The failing control, demonstrated.** With the generation check deliberately
disabled:

```
assertion `left == right` failed: a revised script takes effect on the next write
  left: String("v1")     right: String("v2")
```

Restored, 8/8 green. A cache whose invalidation has never been seen to fail is a
cache nobody should trust.

## Criterion 3 — every bump site, enumerated — and two corrections to my own record

| site | mutates script state | bumps |
|---|---|---|
| `POST /doctype` (create) | yes — whole record incl. `server_script` | ✅ |
| app install / update | yes — `server_script` from the manifest | ✅ |
| app disable / uninstall | yes — detaches metadata | ✅ |
| schema sync (`MetadataSync::sync`) | yes | ✅ |
| `POST /doctype/{name}/script` | **`client_script` only** | ❌ → **fixed** |

**Correction 1: WO-047 said "`POST /doctype/{name}/script` already bumps the
generation." It was wrong twice.** That route handles the **client** script
(WO-017 item 4), not the server script — and it called `invalidate_meta`
**nowhere**. I asserted it from the WO's own framing instead of reading the
route, which is the thing this project's standing checks exist to prevent.

**Correction 2: there is no server-script save route at all.** Server scripts
arrive only via `POST /doctype` and app install/update. Both bump, so the
cache's correctness never depended on the route I misnamed — but the reasoning
that got there was unearned, and the record is now right.

The missing bump was added anyway. It is harmless today (nothing caches
`client_script`) and a landmine tomorrow: this WO's cache rides the *same*
generation, so the day `client_script` joins the cached metadata, that omission
becomes a stale-script bug with no obvious cause. One line now beats an
archaeology session later.

## Criterion 4 — the number

**The property, by WO-047's own trace method** (same instrument, confound-free —
a call either belongs to a request's trace or it does not):

| | WO-047 | WO-048 |
|---|---|---|
| root calls per steady-state write | `{1: 20}` | **`{0: 20}`** |
| session calls per write | `{1: 20}` | `{1: 20}` |

**The steady-state write path now issues exactly one DB call — the write
itself, under the caller's own session.**

**Throughput, controlled A/B**, same fresh database, cache toggled by a
temporary control arm (since removed):

| mode | cache | req/s | p50 |
|---|---|---|---|
| jwt | off | 334.8 | 24.5 ms |
| jwt | **on** | **338.9** | 23.2 ms |
| basic | off | 70.0 | 107.4 ms |
| basic | **on** | **324.9** | 24.0 ms |

- **jwt: no meaningful change (334.8 → 338.9, within noise).** Reported as the
  WO asked: at ~300 µs a root call, the write path is DB-bound elsewhere. The
  claim of this WO is the property, not a throughput headline, and the number
  says so.
- **basic: 70.0 → 324.9 req/s, 4.6×**, p50 **107.4 → 24.0 ms**. This is the
  escape-hatch mode where a root call costs 16.5 ms, and it was dominating.
- **Corroboration worth more than either row:** with zero root calls per write,
  **basic (324.9) and jwt (338.9) converge** — the root-auth mode stops
  mattering for writes, which is the property restated in a second instrument.

**A measurement I refused to report.** The first throughput run, on the live dev
store, read jwt 287 req/s against WO-044's 552 — a 2× *drop* that removing a
query cannot cause. That is the documented surrealkv-WAL-churn caveat (WO-019:
perf gates need a fresh store), not this change. Re-run on a fresh database with
a real cache-off control; the churned number is not reported as a result.

The control arm was **removed** rather than kept: unlike the Desk's
`FRUST_DESK_MAX_INFLIGHT` (which is a correctness control whose failure mode
must stay reproducible), an env var that disables a cache is a prod footgun with
no compensating benefit. The correctness control that matters — breaking
invalidation — lives in the test's design and was demonstrated above.

## Criterion 5 + 6 — both auth modes, no regression

The WO-047 lesson is one WO old, and this change interacts with root auth
directly, so both modes are the floor rather than a nicety.

| | binaries | passed | failed |
|---|---|---|---|
| full kernel suite, parallel, `FRUST_ROOT_AUTH=jwt` | 53 | **331** | **0** |
| full kernel suite, parallel, `FRUST_ROOT_AUTH=basic` | 53 | **331** | **0** |

331, not WO-047's 330: the new out-of-band caveat test. Desk unit tests 4/4;
browser suites green — workflow 18/18, SSE 8/8, print 24/24, mail 15/15.

## Scope held

The idle tickers (5.0/s `claim_next`, 4.0/s `MailWorker::queued_ids`) were left
alone as the WO directed — 9/s at ~300 µs is ~2.7 ms/s. The script pool's
text-comparison semantics (WO-019 c6) are untouched: the cache feeds the pool,
it does not replace it.

## Dev-store note

A scratch database `wo048bench` was created for the fresh-store A/B and
**dropped** afterwards. The live `skeleton` tenant was not used for any number.

## Related
[[WO-048 Server-Script Cache]] · [[2026-07-31 WO-047 hygiene bundle]] (the census
this collapses, and the record it corrects) · [[2026-07-31 WO-044 root jwt auth]] ·
[[2026-07-26 WO-026 surrealdb write concurrency]] (the cache shape) ·
[[ADR-007 Tier-2 Script Architecture]] (the live-mutability property) ·
[[ADR-008 Data Shape]]
