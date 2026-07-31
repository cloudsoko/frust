---
tags: [frust, build-log, apps, plugins, migration, work-order]
created: 2026-07-26
work-order: "[[WO-019 App Lifecycle]]"
status: criteria 1-3 done — WO active
---

# Build Log — WO-019 criterion 3: The Install Story

Install, enable, disable and update through the kernel's manager surface, with
REQ-6.6's gate doing bundle duty and the audit trail coming free from storage.

## OPERATOR NOTE — meta schema 2 → 3

The app registry is meta, so `META_SCHEMA_VERSION` bumps to **3**. Existing
databases — **including the dev `skeleton` DB** — refuse boot until one
`frust serve --accept-meta-migrations` run. That is the fail-closed contract
working, not a nuisance.

**No JWT rotation this time, and no session loss.** WO-008's bump taught
operators to expect signed-out users at a meta bump; this one does not cost
that. Verified, not assumed: `app_registry_ddl()` emits statements against
`installed_app` only — it contains no `DEFINE ACCESS` and does not mention
`app_user` — and `access_ddl()` remains `IF NOT EXISTS`, which is the clause
that stops a re-assert from minting a fresh JWT key (WO-008 Finding A).

**Exercised on the real dev DB, both halves:**

```
without the flag →  E_META_MIGRATION_PENDING: database meta-schema v2 -> v3
                    requires --accept-meta-migrations        (boot refused)
with the flag    →  meta_applied: true, meta_version: 3      (boot complete)
after           →  login OK, 6 doctypes visible, registry reachable,
                    _frust_meta:schema.version = 3
```

**Budget the time.** The accepting boot took ~25 s between `boot_complete` and
`rest_listening` on this substrate — meta migration plus a full metadata sync.
Long enough that a health check probing at 5 s will call it dead; it is not.
Worth knowing before someone wires a restart loop around it.

## The lifecycle

**Install = validate → plan → gate → apply → registry record**, in that order,
with no step running if an earlier one refused. The preview an operator sees is
the real DDL, permission clauses and identity guard included — not a summary of
it.

| Behaviour | Proof |
|---|---|
| schema applies, metadata attaches | table created; doctype record carries `app: "acct"` and the client script |
| installing an installed app | refused: *"app 'acct' is already installed at 1.0.0; **use update**"* |
| update must advance | `1.0.0 → 1.0.0` refused: *"does not advance the installed version"* |
| update adds a field | `memo` exists after `1.1.0`, no restart |
| destructive update | **refused, naming the casualty** |
| manager-only | clerk refused on `/app`, `/app/plan`, `/app/install`, `/app/{n}/disable` |

### The gate names what it protects

```
refused: this bundle performs destructive changes and was not acknowledged:
- REMOVE FIELD memo
```

A gate that only says "no" trains operators to pass the flag reflexively; one
that says *what it is protecting* gets its acknowledgments read. The refused
attempt left `memo` in place (asserted), and the acknowledged run reports what
it destroyed rather than succeeding silently.

## Detach without data loss

The criterion's load-bearing claim, asserted rather than implied:

- disable clears the app's client scripts and flips `enabled`
- **the table and its rows are untouched** — `data_removed: false`, and the
  seeded row is still there afterwards by count and by value
- enable **restores from the stored manifest** — the test asserts the exact
  script text returns and that the row was never re-created

That last point is the difference between a lifecycle and a re-install: because
the manifest is stored verbatim on the registry record, the registry is the
**system of record for what an app is**. Enable is a restoration, not a
reconstruction.

This is the same machinery uninstall will use (criterion 5), so the honest
answer written there can describe what the code does: *metadata detaches, data
remains.*

## Audit is a property of storage

`installed_app` is `CHANGEFEED 30d`, so install / disable / enable / update all
land in the feed by virtue of being writes — **five entries** observed for four
actions. Nothing logs anything; nothing can forget to. More importantly nobody
can skip it, which is the P-5.4 failure this project already refused once
(ADR-002 §7: changefeeds are unbypassable).

## Where things live, per the ruling

- registry DDL → `meta.rs` (kernel-owned meta, binary-authoritative)
- registry **writes** → `rest.rs` (already the metadata-write surface, already
  allowlisted for query text)
- `app.rs` stays **query-free**, so `surql_monopoly` continues to cover it

## Notes

- `Rest::route_for_test` added: a deliberately narrow seam that skips only the
  token→Caller lookup, so every route below it — `require_manager` included —
  runs exactly as for a real request. A seam that keeps the enforcement path
  live is a proof; one that bypasses it is a fixture.
- `Rest::route` was split into `route` (session resolution) and `dispatch`
  (routing), which is what made the above possible without duplicating the
  table.
- `Db::cfg` became `pub` so a bundle can build its own `EngineCtx`.

## Suite state

**30 binaries green, zero failures** (exit 0), stack stopped — `app_lifecycle`
is the 30th. The run now exceeds ten minutes: two DB-heavy binaries joined the
suite this WO, and every app test provisions its own database and runs a real
migration. Worth watching rather than acting on yet; the cost is real work, not
waste.

Scratch databases dropped at close.

## Next

Criterion 4 — routes over REST: bearer discipline, trace spans, tenant
attribution, and WO-013 throttling proven to apply to plugin routes (they must
not be a throttle bypass). **The `route == broker` equality from criterion 1
must survive the dispatch wiring** — that assertion is what catches drift once
dispatch code sits between the route and the broker.

## Related
[[WO-019 App Lifecycle]] · [[2026-07-26 WO-019 criterion 1 the door probe]] · [[2026-07-26 WO-019 criterion 2 the manifest]] · [[ADR-002 SurrealDB Lock-In]] · [[SRS]] (REQ-2.1.1, REQ-6.6.1, REQ-6.6.2)
