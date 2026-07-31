---
tags: [frust, build-log, dr, security, surrealdb, ops, work-order]
created: 2026-07-27
work-order: "[[WO-027 Backup Restore DR]]"
status: complete — DR mechanism confirmed, restore path secured by construction, ADR-013 accepted
---

# Build Log — WO-027: Backup / Restore / DR

The adoption-era risk flagged 27 work orders ago (*"backup/restore, PITR,
monitoring younger than the MySQL ecosystem — define the backup story early"*),
closed. The mechanism works; its one sharp edge now fails loud.

## Criterion 1 — the probe, and what it cost to run honestly

Probed rather than assumed, and the probe returned **two P0s that had nothing to
do with backup**:

1. **[[2026-07-27 WO-028 full-document hooks]]** — a workflow transition was
   silently destroying embedded child rows *before* any backup ran. The DR probe
   noticed because it was the first thing to read a record back closely enough
   to see `lines: []`. Fixed in its own WO; this one paused for it.
2. **The signing-key hole below** — a security P0 in the restore path itself.

Neither would have surfaced from "confirm `surreal export` works."

### What the tooling actually offers

| question | answer |
|---|---|
| full backup | `surreal export`, **per-database** — no whole-instance command; a full backup enumerates namespaces and databases |
| what it captures | tables, `DEFINE EVENT` (lattice, identity guard, Tier-1 counters), `CHANGEFEED` clauses, `DEFINE ACCESS`, users, all rows **including embedded children** |
| restore fidelity | **exact** — see below |
| PITR / incremental | **none.** Snapshot only |
| live backup | yes (it is a read), but it is a snapshot at an instant, not a coordinated one |

### The round-trip, reconciled

Against the accounting seed with real data — 2 approved invoices, embedded
lines, AR rollup, app registry, sessions, meta:

| | source | restored |
|---|---|---|
| invoices | 2 | **2** |
| child line rows | 4 | **4** (contents intact: `49.98`, `1.00`) |
| AR rollup | `101.96` | **`101.96`** exact |
| meta version | 4 | **4** |
| app registry | acct 1.0.0 enabled | **acct 1.0.0 enabled** |
| docstatus / workflow | 1 / Approved | **1 / Approved** |

**785 ms export (25.9 KB) · 891 ms import.** The mechanism is sound.

## THE SECURITY FINDING — proven, not inferred

`surreal export` redacts the JWT signing key to the literal `[REDACTED]`.
`surreal import` restores that string **as the key**. The restored instance
boots, logs in, and serves — while accepting tokens **anyone can forge**.

Demonstrated end to end: a token signed with `[REDACTED]` claiming
`app_user:mgr` returned invoice data from the restored store. **Control:** the
same token against the source store returned `401`. *The restore path
introduces the bypass.*

Severity is about *when*: restore happens during an incident, when an operator
is least auditing and most exposing — and **nothing looks wrong**. Silent-wrong,
in the DR path, with an auth-bypass consequence.

## The guard (ADR-013, accepted)

### Detection: the probe overturned the obvious design

**`INFO FOR DB` reports `KEY '[REDACTED]'` for a HEALTHY access too** — verified
against the source store that had just rejected the forgery. Introspection
therefore *cannot* distinguish a real key from the placeholder; a
comparison-based guard would fire on every store or none.

So the guard **tests the vulnerability itself**: mint a token with the published
constant, ask the database whether it accepts it. A second probe showed
acceptance turns *only* on signature validity, so the forged identity is a
**nonexistent** canary record — no real user needed, works on an empty database.

### Proven both directions, live

| store | result |
|---|---|
| restored (placeholder key) | **boot REFUSED** — `FRUST:E_RESTORED_ACCESS_KEY` + remediation |
| healthy (real key) | **boots normally** — no false positive |
| after re-issuing the key | **boots, and the forgery gets 401** |

The middle row matters as much as the first: a guard that cries wolf gets
disabled by the first operator it inconveniences.

### No serve-anyway flag

Per ruling. Unlike `--accept-meta-migrations` (where proceeding is a legitimate
choice), proceeding here *is* serving-compromised.

### The canary

`keyguard_canary` pins the constant as a **property of the running server** — a
placeholder-keyed store must accept the forgery, a real-keyed store must reject
it. A SurrealDB version that changed the redaction string fails the build
instead of silently blinding the guard.

## Findings

### 1. `access_ddl()` does not remediate — caught only by asserting the outcome

My first remediation test used `access_ddl()`, which is `DEFINE ACCESS IF NOT
EXISTS` (WO-008, so ordinary boots never rotate the key and never sign everyone
out). Against an existing placeholder access it is a **no-op**: it returns `OK`
and fixes nothing. A runbook saying "re-run the DDL" would have shipped a remedy
that succeeds at doing nothing.

It was catchable only because the test asserted **the remedy clears the
refusal**, not that the command ran without error. That is the third instance
of one pattern, in three domains — WO-016's epsilon, WO-028's count-only check,
and this: **assert the outcome you need, not the operation you performed.**

A test now asserts the IF-NOT-EXISTS form *fails* to clear the guard.

### 2. `surql_monopoly` caught the guard's own error message

The remediation DDL lived in `keyguard.rs`, which executes only the constant
`RETURN 1;` — so it was not *assembling* query text in the sense the guard
prevents. The invariant is about query text living in known places, not intent:
a module carrying DDL today is one someone executes DDL from tomorrow.

**Moved to `meta.rs`** (which owns `access_ddl()` and exists for static DDL)
rather than added to the allowlist — the third time this session the right
answer was to move the code, not widen the guard. It landed better than it
started: the remediation now sits beside the `IF NOT EXISTS` form it is the
`OVERWRITE` counterpart to, with the no-op explanation where both are visible.
`meta.rs` stays static (`<access-name>` / `<a fresh high-entropy secret>`
placeholders); `keyguard.rs` names the operator's actual access in prose.

## Deliverables

- **The guard** — `keyguard.rs`, gated in `boot_locked` after `identity_ddl()`
  (so the access exists to test) and before anything is served.
- **The canary** — `keyguard_canary`, 4 tests.
- **[[DR Runbook]]** — every command executed against a live app; mandatory key
  re-issue as a numbered step; honest RTO/RPO.
- **[[ADR-013 Signing-Key Integrity at Boot]]** — accepted.

## RTO / RPO, stated honestly

**RPO = the age of your last export.** There is no PITR: changefeed retention is
finite and per-table, record sessions cannot `SHOW CHANGES` (WO-011), and an
export captures the changefeed *definition*, not its history. An operator who
believes they have point-in-time recovery and learns otherwise at restore time
has a disaster on top of a disaster — naming the absence is the runbook's most
important sentence.

**RTO ≈ 1 minute** for this dataset (0.9 s import + key re-issue + ~25 s
accepting boot + verification).

## Suite

**39 test-result groups across 38 binaries, 0 failed, exit 0** — including the
new `keyguard_canary`.

## Related
[[WO-027 Backup Restore DR]] · [[ADR-013 Signing-Key Integrity at Boot]] · [[DR Runbook]] · [[ADR-008 Data Shape]] · [[ADR-002 SurrealDB Lock-In]] · [[2026-07-27 WO-028 full-document hooks]] · [[SurrealDB]]
