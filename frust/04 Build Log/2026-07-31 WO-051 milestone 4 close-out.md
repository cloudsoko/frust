---
tags: [frust, build-log, milestone-4, gate, assessment]
created: 2026-07-31
work-order: "[[WO-051 Milestone 4 Close-Out]]"
status: **M4 DOES NOT CLOSE — one blocker, found by criterion 0's own restore.** *(Superseded 2026-08-01: the blocker is fixed and the verdict re-ran — see [[2026-08-01 M4 close-out re-run]]. This record stays as written; a gate that said no is the vault's evidence that it can.)* The perf gates passed green on a dedicated fresh store (submit 4 ms vs a 25 ms gate; realtime tax 0.05 ms), retroactively earning WO-050's unmeasured claim. But bringing the live stack back up afterwards, **the kernel refused to boot**: `E_BOOT_DB: refusing destructive change(s) ["REMOVE FIELD crm_followup"]`. WO-050's extension uninstall detaches the field from METADATA and leaves the COLUMN, so the next boot's schema sync sees a destructive removal — and `BootOptions` carries only `accept_meta_migrations`, so **there is no boot-time remedy at all**. An extension uninstall can leave a store that will not start. Everything else in the milestone dispositions cleanly; this one item is the gate doing its job.
---

# WO-051 — Milestone 4 close-out: the gate says NO

The WO-034 precedent governs, and it held: a gate that cannot fail is
decorative. This one failed, on the last thing anyone would have checked.

## Criterion 0 — the outstanding gates (GREEN), and what running them found (RED)

The PM was right to refuse WO-050's reasoning. "A loop of one is the same call"
is an inference about performance, and this project's own 640-arithmetic history
says those get measured. Measured, on a **dedicated fresh store** (a second
SurrealDB process over a scratch data dir — the live dev directory was never
swapped, per standing policy):

| gate | result | budget |
|---|---|---|
| hook chain warm median | **0 ms** | 30 ms |
| **submit warm median** | **4 ms** | 25 ms |
| realtime tax | **0.05 ms** (4.29 → 4.36) | 2 ms |

**WO-050's claim is retroactively earned.** 4 ms sits inside the historical
fresh-store band (WO-040 4 ms · WO-041 3 ms · WO-044 2 ms) at 6× headroom. Stated
precisely: the gate reports integer milliseconds, so it cannot resolve a
sub-millisecond cost for the dispatch loop — what it can say, and does, is that
the loop did not move the floor against its budget.

### And then the restore refused to boot

```
E_BOOT_DB: metadata sync errors: [MigrationError { name: "acct__sales_invoice",
  message: "refusing destructive change(s) [\"REMOVE FIELD crm_followup\"]
            — re-run with allow_destructive to apply" }]
```

**BLOCKER — WO-050 gap: an extension uninstall can leave a store that will not
start.**

The mechanism, traced:

1. WO-050's `detach_extensions` removes the extension's field from the DocType
   **metadata record** — correctly, and per WO-019's promise that *metadata
   detaches, data remains*.
2. The **schema column survives**, holding its data — also correct, and the
   whole point of that promise.
3. The next boot's `MetadataSync` diffs record against schema, sees a column the
   metadata no longer declares, and classifies it as a **destructive removal**.
4. `boot` refuses. Fail-closed, working exactly as designed.
5. **`BootOptions { holder, accept_meta_migrations }` has no `allow_destructive`
   flag**, so the refusal has no operator remedy. The kernel will not start again
   until someone edits the database by hand.

Why WO-019's own uninstall never hit this: uninstalling an app's **own** DocType
`DELETE`s the whole metadata row, so the migrator never considers that table and
there is no diff. An **extension** uninstall leaves the row standing minus one
field — which is precisely the case that produces a diff.

**This is the honest-uninstall promise colliding with the destructive-change
guard**, and neither is wrong on its own. The resolution is a ruling, not a
patch: either the sync must tolerate schema columns that metadata no longer
declares (the reading most consistent with "data remains"), or detach must
retain the field as an orphan, or boot needs an acknowledge path. That is
ADR-008-shaped and is **not decided here** — WO-051 is assessment.

**Dev-store remediation, stated:** the live store was restored by re-declaring
`crm_followup` in the DocType metadata with `orphaned_from: 'crm'`, which keeps
the column *and its row data* and makes the diff empty. Boot green, workflow
18/18. That is a manual remediation of one store, **not a fix** — the defect is
untouched in code.

## Criterion 1 — disposition of every M4 item

| item | status | evidence |
|---|---|---|
| Multi-DB tenancy, 4 topologies, per-tenant restore | **DONE** — P-8.1 killed on executed evidence | [[2026-07-29 WO-040 chunk C namespace topologies]] |
| Connection reuse (port-exhaustion ceiling) | **DONE** | [[2026-07-29 WO-041 connection reuse]] |
| Desk admission control (shed 503, not 500) | **DONE** — shed 0→23655, counter-exact | [[2026-07-29 WO-038 desk admission control]] |
| Frust UI re-skin + behavior layer | **DONE** — 3 behaviours, 0 JS added; ceiling named + ruled | [[2026-07-29 WO-042 frust ui re-skin]] |
| Email battery | **DONE** — metadata notifications, contained worker | [[2026-07-30 WO-043 email batteries]] |
| Root-auth argon2 tax | **DONE** — the 124 req/s ceiling was argon2 | [[2026-07-31 WO-044 root jwt auth]] |
| Hygiene bundle (flake root-caused, CSS guard, root-call census) | **DONE** | [[2026-07-31 WO-047 hygiene bundle]] |
| Server-script cache (zero root calls on the write path) | **DONE** | [[2026-07-31 WO-048 server script cache]] |
| Print — interactive half | **DONE** | [[2026-07-31 WO-046 document view]] |
| Print — PDF engine | **DEFERRED BY RULING**, not undone | [[ADR-014 Print Strategy]] — engine activates when WO-043's attachment need is pulled; Chrome-CDP named as default, typst priced |
| Cross-app extension model | **DONE + dogfooded** — P-2.2 killed | [[2026-07-31 WO-050 extension mechanism]] — **carries the blocker above** |
| Frontend posture | **RATIFIED** (posture, not build) | [[ADR-016 Frontend Posture]] — BYO-frontend priced; SSR kit coupled to topcoat #275 |
| **Desk→kernel streaming** | **NOT DONE** | see below |
| Hook-vocabulary widening | **DEFERRED BY RULING** | [[ADR-015 Cross-App Extension Model]] — ordinary REQ-2.2.1 build work; composition landed on `validate` first |

### Desk→kernel streaming — reasoning re-checked, not inherited

The standing claim is *"backlog optimization, not a blocker — Desk concurrency is
already measured clean."* Re-checked against what is on record rather than
repeated: WO-035 measured the Desk tier at ~135 req/s, and WO-038 **re-recorded
it to ~146** after `spawn_blocking` removed the 16-worker cap — with 48 SSE
subscribers and page load coexisting, 0 failures, neither starving. WO-032's
finding stands that SSE *relocates* polling from the browser to the kernel rather
than eliminating it. So the reasoning holds **for the reason originally given**:
the concurrency question it would improve is measured and clean, and what remains
is drain-loop efficiency. Confirmed not-a-blocker; carried to M5 as an
optimization.

### The DB write ceiling, stated as it now truly is

The old *"~150 w/s raw DB ceiling"* (WO-026) is superseded and should stop being
quoted. WO-044 showed the 124 req/s app-level ceiling was **argon2, not the
database**. WO-048 then showed that removing the last per-request root call moved
JWT-mode throughput **not at all** (334.8 → 338.9) — the path is DB-bound
somewhere else.

Measured today, fresh store, this build:

| concurrency | 1 | 2 | 4 | 8 | 16 |
|---|---|---|---|---|---|
| req/s | 233 | **339** | 332 | 327 | 336 |
| p50 | 4.2 ms | 5.8 ms | 12.0 ms | 24.1 ms | 35.7 ms |

**Saturates at c=2 around ~335 req/s**, throughput flat while p50 grows linearly
with concurrency — the signature of a DB-bound path with idle kernel workers.

**Without inflation:** there is no single authoritative ceiling number. Fresh-store
readings on this machine have ranged **~335–555 req/s** (WO-044 measured ~555 on
a fresh database under a lighter load). The *shape* is stable and reproducible;
the absolute figure is machine-state sensitive. The honest statement is the
shape plus the band, and anyone quoting one number should say which run.

## Criterion 2 — scorecard coherence

- **Tally = rows: 24 · 10 · 0.** The PM's correction stands and the tally line
  now records that it was corrected — a sum that can silently diverge from its
  rows is the plausible-wrong-number class.
- **No bounded-by-assumption has re-crept.** The v2.0 gate's three assumption-class
  bounds (A1/P-2.2, A2/P-7.3, A3 Desk concurrency) were all closed with
  measurement in WO-035/036, and P-2.2 has since been *killed by building*. Every
  surviving bounded verdict carries either a measured number (P-8.2 at 1.4×) or a
  named architecture trade.
- One residue worth stating: **P-2.2's kill is honest but not total**, and its row
  says so — composition is on `validate` only, extension-to-extension ordering is
  out of v1.

## Criterion 3 — verdict

**M4 DOES NOT CLOSE. One blocker:**

> **An extension uninstall can leave a store that will not boot**, with no
> operator remedy short of hand-editing the database. Found by this WO's own
> restore, not by a test — because no test restarts the kernel after an
> extension uninstall.

Everything else disposes cleanly: eleven items done, two deferred **by ruling**
(engine, vocabulary — both stated as rulings rather than omissions), one
not-done and re-justified (streaming).

The blocker is small, well-understood, and its resolution is a short ruling plus a
narrow fix. It is also exactly the class this milestone spent itself learning to
catch: a silent-state problem that only appears one process-restart later. **The
gate could say no, and did.**

## M5, seeded from the honest remainder (on the blocker's resolution)

1. **The extension-uninstall boot blocker** — ruling then fix, plus the missing
   regression: *restart the kernel after an extension uninstall*.
2. SSR-native app-building kit — timing coupled to topcoat **#275** (ADR-016).
3. Hook-vocabulary widening — REQ-2.2.1 build work (ADR-015).
4. REST-surface documentation + the additive-only evolution policy — ADR-016's
   priced obligation, first installment.
5. Desk→kernel streaming — optimization, re-justified above.
6. PDF engine — activates on the WO-043 attachment pull (ADR-014).
7. Watch: 6 upstream Topcoat PRs, #275.

## Related
[[WO-051 Milestone 4 Close-Out]] · [[2026-07-31 WO-050 extension mechanism]] ·
[[v1.0 Pain-Point Scorecard]] · [[v2.0 Deployability Gate]] ·
[[ADR-008 Data Shape]] (the migration-policy ruling the blocker needs) ·
[[ADR-014 Print Strategy]] · [[ADR-015 Cross-App Extension Model]] ·
[[ADR-016 Frontend Posture]]
