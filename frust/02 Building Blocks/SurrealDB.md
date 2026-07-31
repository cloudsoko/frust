---
tags: [frust, building-block, surrealdb]
status: adopted
created: 2026-07-23
---

# Building Block: SurrealDB

> [!info] Role in Frust
> The data engine — and much more. SurrealDB is multi-model, so it doesn't just replace MariaDB: it collapses most of Frappe's polyglot stack ([[Frappe Pain Points#2. Architecture & Runtime|P-2.3]]) into one system, and unlocks capabilities Frappe never had. The goal is to exploit **every modality**, not use it as a boring SQL substitute.

## Stack Collapse

| Frappe component | SurrealDB replacement | Pain point killed |
|---|---|---|
| MariaDB | Core multi-model store (document + relational + graph) | [[Frappe Pain Points#1. Performance & Concurrency|P-1.3]] |
| Redis (cache) | Metadata lives in-DB with fast key access; in-memory engine for hot paths | P-1.2 |
| Node + socket.io + Redis pub/sub | **LIVE SELECT** — realtime queries native in the DB | P-2.4 |
| `Version` doctype (audit) | **Changefeeds** / versioned tables — tamper-proof, can't be bypassed by `db_set` | P-5.4, [[SRS#3.2 Auditability|REQ-3.2.1]] |
| `permission_query_conditions` (string-concat SQL) | **`DEFINE TABLE … PERMISSIONS WHERE`** — row-level security evaluated by the DB itself | P-5.3, [[SRS#3. Security & Fine-Grained Access Control|REQ-3.1.2]] |
| Site-per-directory multi-tenancy | **Namespace → Database** hierarchy — native tenancy | P-8.1 |
| `bench migrate` DDL sync | Runtime `DEFINE FIELD/TABLE/INDEX` — schema *is* data, mutable live | P-4.1, [[SRS#1.1 Schema Definition & Persistence|REQ-1.1.2]] |
| MariaDB `LIKE` search | **Full-text search** — `DEFINE ANALYZER` + search indexes, BM25 scoring | P-7.5 (API quality) |
| — (didn't exist) | **Vector indexes** (HNSW/M-Tree) — semantic search, AI features | new capability |
| — (didn't exist) | **Geometry types + geo queries** — native geospatial | new capability |

## Modality → Requirement Map

### 1. Schemafull/Schemaless hybrid → the DocType engine
`DEFINE TABLE` + `DEFINE FIELD` are runtime statements, not migrations. A DocType's metadata can compile directly into `DEFINE` statements: field types, `ASSERT` clauses for validation (required, regex, min/max), `DEFAULT`, `VALUE` for computed fields, unique indexes.
→ [[SRS#1.1 Schema Definition & Persistence|REQ-1.1.1–1.1.3]], [[SRS#1.2 Data Access & Query Engine|REQ-1.2.2]], kills [[Frappe Pain Points#4. Schema & Migrations|P-4.1]]
**Improvement over Frappe:** validation enforced *in the DB*, not in 4 scattered layers (P-3.2). Raw writes can't bypass `ASSERT`.

### 2. Record links & graph edges → Link fields without JOINs
- Record IDs (`customer:xyz`) replace `name` varchar PKs — typed, direct pointers.
- Link fields become record links: `SELECT *, customer.* FROM invoice` — no JOIN, no N+1 (P-1.3).
- `RELATE` edges model many-to-many and *typed relationships with properties* (e.g. `student ->enrolled_in-> course` with date) — Frappe needed junction child-doctypes for this.
- Graph traversal (`->`, `<-`) enables queries Frappe can't express: org charts, BOM explosions, approval chains, dependency trees in one query.

### 3. Nested objects & arrays → child tables, rethought
Child doctypes (`Sales Invoice Item`) can be embedded arrays of objects in the parent record — atomic reads/writes of a full document, no separate table, no `parent`/`parenttype`/`idx` bookkeeping. → kills a whole class of P-1.3.
⚠️ Design decision: embedded vs `RELATE` — see [[#Open Design Decisions]].

### 4. LIVE SELECT → realtime as a primitive
Any query can be live. Desk list views, form updates, kanban boards, notifications — all subscribe to `LIVE SELECT` with permissions applied by the DB. Deletes the entire Node sidecar (P-2.4) and gives realtime *with row-level security*, which Frappe's socket.io never had.

### 5. DEFINE EVENT → in-DB lifecycle tier
Table events (`WHEN $event = "CREATE" THEN …`) give a DB-level hook tier for invariants that must *never* be skipped (audit stamps, denormalized counters). App-level hooks ([[SRS#2.2 Event Hooks & Extension Points|REQ-2.2.1]]) stay in Rust/plugins — events are the bottom layer of defense.

### 6. PERMISSIONS clauses → row/field security in the engine
`DEFINE TABLE … PERMISSIONS FOR select WHERE owner = $auth.id` and `DEFINE FIELD … PERMISSIONS` — both row-level (REQ-3.1.2) and field-level (REQ-3.1.1) security evaluated inside the DB. Combined with record-user auth (`DEFINE ACCESS`), even a compromised app layer can't over-read.
**Improvement over Frappe:** P-5.3's string-concatenated SQL fragments become declarative, testable rules.

### 7. Changefeeds → authoritative audit
`DEFINE TABLE … CHANGEFEED 90d` records every mutation at the storage layer. The audit trail (REQ-3.2.1) becomes impossible to bypass — no `flags.ignore_version` equivalent exists (P-5.4).

### 8. DEFINE FUNCTION → server-side computed logic
Named SurrealQL functions for computed fields, denormalizations, reusable query fragments. Candidate replacement for a slice of Frappe's Server Scripts — declarative, sandboxed by design, no `eval` (P-5.1).

### 9. Full-text + vector + geo → search Frappe never had
- `DEFINE ANALYZER` + FTS indexes: proper global search with relevance scoring.
- Vector indexes: semantic search over documents, RAG over ERP data, "find similar records" — a first-class AI story.
- Geometry types: geofencing, route/territory queries — native (relevant for logistics domains).

### 10. Deployment spectrum → ops simplicity
Single Rust binary. Runs embedded (in-process, SurrealKV/RocksDB), as a server, or distributed (TiKV) — same API. Dev/test can use the **in-memory engine** (fast, isolated tests → kills P-7.2); production scales without changing code. One `frust` binary + one `surreal` binary replaces bench's supervisor zoo (P-7.1, P-2.3).

### 11. Namespace/Database hierarchy → tenancy model
`Namespace` (org) → `Database` (tenant/site) with isolated auth scopes. Multi-tenancy (P-8.x) becomes a DB primitive instead of directory conventions. Feeds the open SRS gap on tenancy.

## Open Design Decisions
*Each of these becomes an ADR in `03 Architecture Decisions/` when decided.*

- [x] **Child tables:** → **decided:** [[ADR-008 Data Shape]] — embedded default, immutable per-child flag for related; storage-agnostic logical access compiled by the broker.
- [x] **Logic placement:** → **decided:** [[ADR-009 Execution Model]] — kernel by default; DB tier admits only via the two-clause test; one resident (docstatus lattice).
- [ ] **Storage engine:** embedded SurrealKV (single-binary deploys) vs client-server vs TiKV. Probably: embedded default, server/TiKV as scale-out path.
- [x] **Tenancy grain:** → **decided:** [[ADR-003 Tenancy Model]] — platform namespace + database-per-tenant, pluggable strategies.
- [x] **Job queue:** → **decided:** [[ADR-009 Execution Model]] — table-as-queue, viable-with-bridge (WO-004: 3,200/3,200, at-commit delivery); the bridge IS the worker; atomic claim is the serialization point. **All six day-one decisions now closed.**
- [x] **Metadata storage:** → **decided:** [[ADR-008 Data Shape]] — self-hosted `doctype` records; binary-authoritative meta, fail-closed boot, explicit-ack meta-migrations.

## Risks & Reality Checks

> [!warning] Eyes open — this is a big bet.
> - **Query planner maturity.** ~~Unproven~~ → **Measured 2026-07-23** ([[2026-07-23 SurrealDB week-1 benchmark]]): every Frappe report shape at interactive speed on 100 k invoices / 500 k embedded lines (register 40 ms, group-by 355 ms, 2-hop traversal 700 ms, 500 k-line flatten 900 ms). **But:** compound range indexes are a 30× trap — planner pushes only the lower range bound into the index, post-filters the rest with a fetch per entry. Query layer MUST expose index hints (REQ-1.2.1); range indexes opt-in, never auto-derived from DocType meta; `DEFINE INDEX … CONCURRENTLY` always. 1 M-row re-run pending before GA-scale claims.
> - **Live query scale.** ~~Verify limits~~ → **Measured ([[2026-07-25 WO-011 live-query scale spike]]):** latency-viable to N=1000 (p50 92 ms), parked subs free at idle, zero leaks/loss — but **writes pay ~70 µs per parked subscription on their table** → ~50/table budget, kernel-enforced ([[ADR-011 Realtime]]). Record sessions refused `SHOW CHANGES` (loud IAM error) — per-subscriber replay impossible DB-direct; reconnect = refetch. **Risk list fully measured: no unverified behaviors remain.**
> - **Changefeed retention cost.** ~~Measure~~ → **Priced at 1 M ([[2026-07-24 WO-006 1M-row scale proof]]):** +2 ms per submit, ~1 doc-copy per write (329→558 B), `SHOW CHANGES` first-100 in 26 ms. Cheap enough to stay always-on; retention windows sized per [[ADR-009 Execution Model]] (queue) and audit policy.
> - **Blocking `DEFINE INDEX` is disk-fatal at scale** (WO-006): >6 GB transient WAL at 1 M, died on os error 112; `CONCURRENTLY` = 41 s, ~1.8 GB. CONCURRENTLY is the *only* viable build path, not politeness.
> - **Q3-shape (per-row 2-hop in aggregation) is super-linear** (19× on 10× data) — live 2-hop aggregates decay with table size; [[ADR-010 Materialized Aggregates]] Tier 2 removes them from the hot path. Upstream watch-item.
> - **Ops tooling.** Backup/restore, point-in-time recovery, monitoring are younger than the MySQL ecosystem. Define the backup story early.
> - **License.** BSL 1.1 — free unless we offer SurrealDB itself as a hosted DBaaS. Fine for an ERP product; re-check if the business model shifts.
> - **Lock-in.** Deep use of PERMISSIONS/EVENTS/LIVE/graph means no realistic DB swap later. Accepted deliberately — the modalities *are* the architecture. → written down: [[ADR-002 SurrealDB Lock-In]].

## v3.2.0 Implementation Caveats (loud/documented quirks — not silent-misbehavior instances)

*Found during WO-005 module 3 integration; recorded so no future sync module rediscovers them.*

- **`FLEXIBLE` on `array<object>` does not propagate to elements** — the element path needs its own explicit `DEFINE FIELD lines.* … FLEXIBLE`. Affects every embedded-children DDL emission ([[ADR-008 Data Shape]]); handled in the kernel's schema module.
- **Root/system sessions have no `$auth`** — owner-typed fields must be `option<record<app_user>>` or root-session writes fail. Affects any field defaulting from `$auth`.
- **`$auth.id` resolves only if the user can select their own `app_user` record** — under `PERMISSIONS NONE` it silently yields NONE: owner stamps NULL, and `owner = $auth.id` clauses pass via `NONE = NONE` for everyone. A row-visibility hole one config drift away → [[WO-008 Identity Hardening]] (kernel-owned `app_user` DDL, loud NULL-identity, null-safe clauses).
- **EVENT-body writes bypass table permissions** — probed in WO-007; *load-bearing* for Tier-1 counters (write-closed rollup tables that EVENTs can still update). Canary pinned (WO-008) in both directions; failure messages name what must be re-ruled before trusting rollups on a bumped pin.
- **Long-lived surrealkv instances degrade under database create/drop churn** (WO-008: gate read 31→43 ms on churn-heavy dev instance; restart restored 22–25 ms). Production doesn't churn databases; test harnesses do — restart the instance before trusting perf numbers.
- **Run the substrate probe before any perf A/B** (WO-010 incident): raw `RETURN 1` against bare surreal must read single-digit ms. A machine-level tax (~37 ms server CPU per trivial request, all local instances affected, store churn ruled out by rebuild) mid-session produced 73–80 ms gate readings that were *not* the kernel's. 5-second probe first; reboot-and-rerun on the checklist; CI's gate is the judge. Calibrate the threshold per client stack (node fetch ≈ +20 ms).
- **Drop scratch databases at the end of every measuring WO** (WO-013: 76 accumulated databases + ~300k noisy rows polluted the gates at 29–43 ms; dropping 75 scratch DBs restored 23–24 ms). Restart alone is NOT enough — churn is cumulative across a session.
- **Perf gates use a DEDICATED THROWAWAY store — NEVER swap the live dev store's data directory** (WO-018/020 incident): the "fresh store for perf gates" rule was practiced by juggling `data`/`data.bak` on the dev store, and a mistimed swap left `skeleton` restored from a bare backup — the WO-002 dev dataset destroyed (recovered exactly only because WO-010 had preserved `data-degraded-20260725`). The third clause of "own invocation" stands, but it means *point the perf kernel at a scratch data-dir*, never mutate the dev store's. Dev data is not a perf fixture.
- **The WAL does not compact — dropping databases is not enough either** (WO-019 c4): surrealkv keeps one append-only WAL; ~500 create/drop cycles left a 50 MB uncompacted file that tripled write latency (submit 34→84 ms, tax 0→5.4 ms) **after** dropping 58 DBs and restarting. Only a fresh data directory restored it (34 ms, tax 0.00). **Perf gates need a fresh store**, not just a quiet machine — this is the third clause of "own invocation." This finding also RESOLVED the all-session realtime-tax flap: it was tracking WAL growth, not subscriptions.
- **v3.2.0 requires ORDER BY fields in the projection** (WO-013; sibling of WO-007's GROUP BY rule) — the query renderer handles it; don't hand-write around it.
- **`DEFINE ACCESS … TYPE RECORD ON NAMESPACE` is rejected (HTTP 400)** — RECORD access must be `ON DATABASE` (the auth query hits the `app_user` *table*, tables live in databases). JWT access *can* be ON NAMESPACE, RECORD cannot. So all tenancy topologies place record access ON DATABASE — a *security gain* under namespace-per-tenant-env (per-environment signing key → a sandbox token can't authenticate against production). WO-040-C keeps `AccessPlacement::Namespace` in the type but `provision.rs` refuses to render RECORD-on-namespace rather than emitting DDL the DB rejects.
- **`surreal import` is ADDITIVE, not restore-over** — fails "record already exists" against a populated database. Per-tenant restore into a live instance = export → `REMOVE DATABASE` → import (see [[DR Runbook]]). WO-027/WO-039 never met it (imported into fresh DBs); WO-040-C did.
- **Unbraced multi-statement batches commit PER-STATEMENT — and the transport's conflict-retry re-runs the WHOLE batch, re-executing already-committed statements** (WO-047, root cause of the `revoke_kills_` "flake": `DEFINE TABLE IF NOT EXISTS; CREATE …` raced on the DDL *after* the CREATE committed → retry duplicated the session row; hid because both rows carried the same token — logins worked, COUNTs lied). **Standing rule: anything on the retry path is single-statement or a braced transaction.** The retry contract ("conflict = didn't commit = safe to retry") is only true of atomic units; an unbraced batch is not one.
- **`surreal export`/`import` = a RESTORE-PATH AUTH BYPASS (proven, not inferred — WO-027)** 🔴 **security-P0.** Export redacts the JWT signing key to the literal string `[REDACTED]`; **import installs that string AS the live signing key.** The restored instance boots, logs in, and serves *normally* — while accepting tokens forged by anyone who knows the constant (i.e. anyone who has seen any SurrealDB export file). PROVEN: a JWT signed with literal `[REDACTED]` claiming `app_user:mgr` was accepted by the restored store and returned invoice data; the source store (real key) returned 401 to the same token. **Nothing looks broken — that's the danger, and it fires exactly when an operator restores under incident pressure and isn't auditing DDL.** Frust's answer: fail-closed boot guard (`FRUST:E_RESTORED_ACCESS_KEY`, no serve-anyway ack, canary-pinned detection — [[WO-027 Backup Restore DR]] / ADR-008 lineage) + mandatory runbook key re-issue. Also: export is per-database (whole-instance = enumerate ns/db). Round-trip fidelity otherwise exact (785/891 ms, 25.9 KB; AR 101.96, 4 line rows intact post-WO-028).
- **Concurrent `DEFINE DATABASE` conflicts** (WO-025) — parallel test setup racing the same DB name throws write-conflict; `IF NOT EXISTS` narrows but doesn't remove it. Use a unique DB name per call (same fix as the WO-013/WO-016 parallel-fixture races).
- **SurrealDB write concurrency is the post-WO-025 throughput ceiling** — with the serve loop parallel, `db_write` averages ~222 ms/call and surreal burns ~10× the kernel's CPU; the bound moved from the kernel to the DB. This is where the next scale conversation happens (connection pooling? write batching? an ADR-003 per-tenant-process question), and it's the honest place for the ceiling to sit.
- **Type migrations leave BOTH value shapes live in one table** (WO-017 item 1): rows written before an `ALTER FIELD` (e.g. Currency float→decimal) keep their old serialization; new writes get the new one. Any reader must handle both until a backfill rewrites history — `as_f64().unwrap()` on a "migrated" column is a latent panic. True of every deployment that migrates rather than starts fresh.
- **A permission-refused UPDATE returns an EMPTY result set, not an error** (WO-018 Finding A): SurrealDB filters the write set by row permission, so a write the caller may not perform matches zero rows and returns `Ok([])` — success-shaped. **Check rows-affected; never trust `Ok` alone on a write.** This is SurrealDB behaving *correctly* (documented), so it is NOT an ADR-002 silent-misbehavior instance — counter stays at 2 — but Frust swallowing it was a silent write failure, now typed `E_WRITE_NO_ROWS`. It masked [[WO-020 Row-Write Permission]]'s Finding B for 13 WOs: enforcement was invisible in both directions.
- **Re-defining a `DEFINE ACCESS` mints a fresh JWT key** — logs out every session. Kernel uses `IF NOT EXISTS` on boot re-assert; key rotation happens only at deliberate meta-migrations.

## Related

- [[Frust Hub]] — project home
- [[Frappe Pain Points]] — what this building block kills
- [[SRS]] — requirements it serves
