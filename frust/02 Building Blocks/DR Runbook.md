---
tags: [frust, runbook, dr, ops, surrealdb]
status: tested 2026-07-27 (WO-027) — every command here was run against a live app
created: 2026-07-27
---

# Runbook — Backup, Restore, Disaster Recovery

> Every step below was executed against a running kernel with a real app
> installed (the accounting seed: DocTypes, workflow, server script, child
> tables, Tier-1 rollups), and the restored numbers were reconciled against the
> source. Measured figures are from that run.

## Scope

The **two-process deployment**: `frust` + `surreal.exe` on surrealkv. Multi-node
/ distributed (TiKV) DR is out of scope and untested.

## 1. Backup

```bash
surreal export --endpoint http://127.0.0.1:8899 -u root -p root \
  --ns frust --db skeleton  backup-YYYYMMDD.surql
```

**Export is PER-DATABASE.** There is no whole-instance command — a full backup
enumerates namespaces and databases and exports each. Script it; do not assume
one file is everything.

What the dump contains (verified): table definitions, `DEFINE EVENT` (the
docstatus lattice, identity guard, Tier-1 counters), `CHANGEFEED` clauses,
`DEFINE ACCESS`, users, and all row data including **embedded child tables**.

**Measured** (2 invoices, 4 child rows, app registry, meta, sessions):
**785 ms, 25.9 KB**. Cost scales with data, not with schema.

### Can it run against a live kernel?

Yes — `export` is a read. It is a **snapshot at an instant**, not a coordinated
one: writes landing mid-export may or may not be included. For a consistent
point, quiesce writes or accept the boundary. There is **no PITR** and no
incremental backup (see §5).

## 2. Restore

```bash
# 1. bring up a clean store
surreal start --user root --pass root --bind 127.0.0.1:8899 \
  surrealkv://<fresh-data-dir>

# 2. import
surreal import --endpoint http://127.0.0.1:8899 -u root -p root \
  --ns frust --db skeleton  backup-YYYYMMDD.surql
```

> [!danger] `surreal import` is ADDITIVE, not restore-over (WO-040-C, 2026-07-29)
> Import into a database that **already holds data fails** ("record already exists") — it merges, it does not replace. The whole-instance path above is safe *only because the store is fresh*. **Per-tenant restore into a LIVE multi-tenant instance is THREE steps: export → `REMOVE DATABASE <tenant>` (drop the target) → import.** A restore script without the drop fails halfway through an incident — exactly when nobody can debug it. This was invisible to WO-027/WO-039 because both imported into fresh databases and never met a populated target. Verified: tenant A exported → corrupted → dropped → re-imported, tenant B's post-export write intact.

**Measured: 891 ms.** Verified restored, exactly: invoice count, **embedded
child rows with contents intact**, AR rollup `101.96` to the cent, meta version,
app registry (`installed`/`enabled`/version), docstatus and workflow state.

## 3. ⚠ MANDATORY: re-issue the signing key

**This is a step, not a footnote. Skipping it leaves the instance open to
anyone.**

`surreal export` redacts the JWT signing key to the literal string
`[REDACTED]`, and `surreal import` restores that string **as the key**. A
restored instance authenticates fine — and accepts tokens forged by anyone who
knows the constant. *Proven:* a forged token returned application data from a
restored store; the same token got `401` from the source.

```sql
USE NS frust DB skeleton;
DEFINE ACCESS OVERWRITE account ON DATABASE TYPE RECORD
  SIGNIN (SELECT * FROM app_user WHERE name = $name
          AND crypto::argon2::compare(pass, $pass))
  WITH JWT ALGORITHM HS512 KEY '<fresh high-entropy secret>'
  DURATION FOR TOKEN 1h, FOR SESSION 12h;
```

- **`OVERWRITE` is required.** The kernel's own `access_ddl()` is
  `IF NOT EXISTS` (so ordinary boots never rotate the key and never sign
  everyone out) — against a restored access it is a **no-op** and fixes
  nothing.
- **Sessions never survive a restore.** The key changes, so every outstanding
  token is invalid. Users log in again. This is unavoidable, not a bug.

**The kernel enforces this.** `frust serve` refuses to boot a store that accepts
the placeholder key, with `FRUST:E_RESTORED_ACCESS_KEY` and this DDL in the
error. There is deliberately no flag to serve anyway (ADR-013). The runbook step
exists because an older kernel, or a non-Frust consumer of the same database,
still needs the procedure — guard enforces, runbook documents, neither alone is
sufficient.

## 4. Bring the kernel up

```bash
frust serve            # refuses if the key was not re-issued
```

If the meta-schema version moved between the backup and the binary, boot also
requires `--accept-meta-migrations` (ADR-008). Budget for it: **an accepting
boot took ~25 s** between `boot_complete` and `rest_listening` on the 1 M
fixture — a health check probing at 5 s will call a working kernel dead.

## 5. RTO / RPO, stated honestly

| | value | basis |
|---|---|---|
| **RPO** | **the age of your last export** | snapshot-only; no PITR, no incremental |
| **RTO** (this dataset) | **≈ 1 min** | 0.9 s import + key re-issue + ~25 s boot + verification |
| **RTO** (scaling) | import time grows with data | 891 ms for 26 KB; benchmark your own volume |

**There is no point-in-time recovery.** The changefeed is a *mutation log* and
is tempting as one, but it is **not a recovery tool here**: retention is finite
and per-table (`CHANGEFEED 1d`/`7d`/`4w`), record sessions cannot `SHOW CHANGES`
(WO-011), and an export captures the changefeed *definition* — not its history.
Treat it as the audit trail it is (REQ-3.2.1) and set RPO from export cadence.

## 6. Verify a restore (do this every time)

```sql
USE NS frust DB skeleton;
SELECT count() FROM <your-doctype> GROUP ALL;          -- row counts
SELECT VALUE math::sum(array::len(<child-field>)) FROM <your-doctype> GROUP ALL;  -- child rows
SELECT * FROM <your-rollup>;                           -- aggregates reconcile
SELECT version FROM _frust_meta:schema;                -- meta version
SELECT name, version, enabled FROM installed_app;      -- apps still installed
```

**Check child/embedded fields explicitly.** A WO-028 bug once emptied them
before backup, and a count-only check would have called that restore perfect —
*a verification that only checks what it expects cannot catch what was
silently destroyed*.

## Caveats index

1. Export/import are **per-database**; no whole-instance command.
2. The signing key is **redacted on export and restored literally** — §3 is
   mandatory (ADR-013).
3. **No PITR / no incremental.** RPO = export interval.
4. Sessions never survive a restore.
5. An accepting boot can take ~25 s — do not let a health check kill it.
6. surrealkv's WAL does not compact; a long-lived store degrades write latency
   (~3×). Restoring into a fresh store also resets that.

## Related
[[ADR-013 Signing-Key Integrity at Boot]] · [[ADR-008 Data Shape]] · [[SurrealDB]] · [[WO-027 Backup Restore DR]] · [[2026-07-27 WO-028 full-document hooks]]
