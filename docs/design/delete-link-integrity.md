# Delete link integrity

Status: proposed for a future implementation; no runtime behavior changes in this document.

## Decision

Compile every metadata `Link` field into a SurrealDB record reference and make
its deletion policy explicit. An omitted policy means `refuse`; the first
implementation accepts only `refuse` and emits a synchronous database-side
reference action that aborts the target deletion with a stable
`FRUST:E_LINK_EXISTS` code. SurrealDB, not the application door, maintains the
reverse-reference catalog on writes and consults it atomically on delete.

In schematic SurrealQL, the compiled field is:

```surql
DEFINE FIELD customer ON sales_invoice
  TYPE option<record<customer>>
  REFERENCE ON DELETE THEN {
    THROW 'FRUST:E_LINK_EXISTS:sales_invoice:customer';
  };
```

The exact error payload and the meanings of the reference action's `$this` and
`$reference` values must be pinned by a capability test before implementation.
If the pinned database cannot produce a stable machine-readable refusal from
`ON DELETE THEN`, use `REFERENCE ON DELETE REJECT` only if the pinned server
exposes a stable typed error that the broker can translate. Otherwise stop the
implementation; do not fall back to an application-side preflight scan.

This is one compiler rule. `on_delete` belongs to Link metadata, the metadata
compiler emits the database clause, and every ordinary database writer pays the
same invariant. There is no second policy engine in REST and no new runtime
lock.

## Existing floor

The public delete door currently sends one caller-session `DELETE ... RETURN
BEFORE` statement. Compiled table permissions decide who may delete, and
database events decide which records may be deleted
(`frust-kernel/kernel/src/broker.rs:635-682`). Delete permission is currently
manager-only (`frust-kernel/kernel/src/sync.rs:352-389`). The API documentation
is explicit that Link fields are not checked and dangling links are possible
(`frust-kernel/docs/rest-api.md:266-284`).

The necessary reverse graph is already data, not convention. A field definition
contains `fieldtype` and `options` (`frust-kernel/kernel/src/sync.rs:65-98`), a
Link's first option is validated and compiled as a typed target record
(`frust-kernel/kernel/src/sync.rs:443-453`), and Link targets already become
resource dependencies (`frust-kernel/kernel/src/sync.rs:489-509`). The design
therefore adds a policy to a graph the compiler can already compute; it does not
introduce a handwritten registry.

The database version is part of the contract: this repository pins SurrealDB
3.2.3 (`README.md:72-75`). SurrealDB record references have supported
`ON DELETE REJECT`, `UNSET`, `CASCADE`, and custom `THEN` actions since 2.2.
Reference actions are the relevant primitive because they maintain incoming
references in the database; ordinary record links alone deliberately permit
dangling values. See the SurrealDB documentation for
[record references](https://surrealdb.com/docs/reference/query-language/language-primitives/record-references)
and the [3.2 delete-path note](https://surrealdb.com/releases/3.2).

## User-visible semantics

The default is **refuse**. Deleting a record with at least one live incoming
Link changes nothing and returns a typed conflict containing one bounded
witness: target id, referring DocType, referring field, and referring record id
when the database exposes it safely. The proposed HTTP shape is `409
link-exists`, code `FRUST:E_LINK_EXISTS`; clients branch on the kind and code,
not human detail. The contract already models stable delete refusal codes
(`frust-kernel/kernel/src/contract.rs:173-214`), although the implementation may
choose a distinct `LinkExists` variant because current `DeleteRefused` maps to
422 (`frust-kernel/kernel/src/rest.rs:2817-2825`).

Only one witness is returned. Enumerating every reference makes refusal latency
and response size proportional to fan-in and may disclose records the caller
cannot read. The operator removes or retargets references explicitly and
retries. There is no public `force` flag.

This matches Frappe's safe default and useful error: Frappe discovers reverse
Link fields, queries each referring DocType, and raises `LinkExistsError` with a
referring document (`frappe/model/delete_doc.py:278-355` in
[Frappe's delete implementation](https://github.com/frappe/frappe/blob/develop/frappe/model/delete_doc.py)).
Frust does not copy Frappe's application-side loop, row lock, dynamic-Link
special cases, ignore-hook list, or force bypass
(`frappe/model/delete_doc.py:137-169`). Those are the casualties of keeping one
compiled enforcement floor.

### Why cascade and nullify do not ship first

| Policy | Benefit | Casualties | Decision |
| --- | --- | --- | --- |
| Refuse | No invisible mutation; bounded work at delete; Frappe-compatible safety | Operators must unlink deliberately; self-links must be removed first; some deletes become multi-step | Default and only first-stage policy |
| Cascade | Convenient ownership cleanup | Secondary deletes have no fresh door-level authorization decision; interaction with permissions and docstatus/Single guards is unproven; fan-out, cycles, per-row events, audit volume, and latency become data-dependent | Reserved, rejected by sync until separately designed |
| Nullify (`UNSET`) | Preserves referring documents | Silently changes business records; required Links turn the operation back into refusal; may mutate submitted/otherwise immutable rows; fan-out updates and events are unbounded | Reserved, rejected by sync until separately designed |

The house style is honest refusal: if the system cannot prove that every
secondary mutation is authorized, bounded, observable, and compatible with
lifecycle events, it names the blocker and changes nothing. Cascade's casualties
are the referring rows; nullify's casualties are their Link values and the
history those values represented. Refuse's casualty is convenience, which is
the only reversible loss of the three.

## Alternatives and cost model

Let `L(T)` be the number of metadata Link fields whose target is DocType `T`,
`N_i` the row count of referring table `i`, `R` the number of non-empty Link
values in the tenant, and `k` the number of Link values changed by one source
write.

### Door-side reverse scan

The straightforward door algorithm issues, for every incoming field, `SELECT
id FROM source WHERE link = $target LIMIT 1`, then deletes if all queries are
empty.

Without indexes, worst-case work is `O(sum(N_i))` per delete. At one million
rows, every unindexed incoming field can require a million-row scan, and `L(T)`
such fields multiply it. The repository's nearest scale evidence is deliberately
not treated as a direct delete benchmark: a live aggregate scan over the 1M-row
fixture was about 7.7 seconds (`frust-kernel/kernel/tests/aggregates_ladder.rs:432-433`,
`frust-kernel/kernel/tests/aggregates_ladder.rs:486-498`). It is enough to rule
out an unindexed scan from a synchronous door whose existing release write
budget is 25 ms (`frust-kernel/kernel/tests/perf_gates.rs:20-25`).

With one ordinary index per Link field, lookup becomes approximately
`O(L(T) * log N_i)` plus at most one row fetch per field. Space becomes
`O(R)`, and each changed Link adds index maintenance to the source write. This
is viable only if sync owns every index and `EXPLAIN` proves every emitted query
uses it; SurrealDB warns that a predicate mismatch falls back to a full scan and
that indexes add write cost. See
[DEFINE INDEX](https://surrealdb.com/docs/reference/query-language/statements/define/indexes).

Even indexed, an application preflight followed by the current single delete
has a check/delete race. Closing it requires a transaction whose conflict
behavior is proven against concurrent Link creation, or a new lock. It also
adds `L(T)` policy queries to the door and must decide whose read permissions
apply. That is precisely the policy and hot-path sprawl this design rejects.

### Generated target-table event

The compiler could generate a delete event on each target table containing the
same indexed queries. A synchronous SurrealDB event runs inside the triggering
transaction, so `THROW` can roll back the delete; Frust already uses that shape
for docstatus and Single identity (`frust-kernel/kernel/src/sync.rs:132-141`,
`frust-kernel/kernel/src/sync.rs:186-198`). This removes the application race and
keeps policy in generated database clauses.

It does not remove scan cost. It still performs up to `L(T)` index lookups for
each deleted row, makes the target resource's DDL depend on every referring
resource, and a multi-row `DELETE` evaluates the event per row. SurrealDB also
evaluates a multi-row `UPDATE` and its events per row, so using source events to
maintain state moves the multiplier onto bulk writes. Adding or removing a Link
field must regenerate a different table's event. This is a correct fallback
only if native reference tracking fails its capability gate; it is not the
recommendation.

### Hand-built reverse table

A generated source-table event could maintain one hidden reverse row for each
non-empty Link, while a target delete event checks that table. The required
invariant is biconditional: after every committed create, update, or delete,
`source.field = target` exists if and only if exactly one reverse entry
`(target, source, field)` exists. Both sides must change in the same transaction;
async repair is insufficient because a delete could pass during the gap.

That shape costs `O(k)` additional event writes and `O(R)` storage, and bulk
source updates multiply the work per row. It also owns collision-free keys,
backfill, drift detection, rebuild, import behavior, and reconciliation forever.
Frust's existing counter events demonstrate that event-maintained state is paid
on every mutation (`frust-kernel/kernel/src/sync.rs:201-230`). Reimplementing a
database feature with the same cost shape creates more invariant surface, so it
is rejected.

### Native database-maintained references — recommended

`REFERENCE ON DELETE THEN { THROW ... }` is the generated reverse index, write
maintenance, and atomic refusal in one schema clause. Source writes pay for the
Link values they change; target deletes consult the maintained incoming
references instead of scanning all source rows. The existing delete remains one
caller-session statement, compiled permissions still decide **who**, and the
reference clause decides **whether this target is still in use**. No process
mutex, distributed lock, root-session preflight, or second application compiler
is added.

The invariant is:

> For every committed non-empty metadata Link `S.f = T:id`, the database's
> reference catalog contains the matching incoming reference before any target
> deletion can commit; removing or changing `S.f` removes the old entry in the
> same transaction.

Native support does not make cost free. It moves work from rare deletes to all
Link writes and consumes storage proportional to `R`. The 3.2 release notes say
`DELETE` skips its reference purge scan only when schema proves that references
cannot exist, so both the linked and unlinked target paths require measurement.

## Cost budget

These are acceptance gates for the future implementation, not measured claims:

| Path on the 1M-row fixture | Budget |
| --- | --- |
| Write that changes no Link | Existing 25 ms warm release median remains green; added median tax <= 1 ms |
| Create/update changing one scalar Link | Incremental median <= 2 ms and p95 <= 5 ms versus the same write with `NONE` |
| Delete an unreferenced target with 1M maintained references distributed across other targets | Database-operation p95 <= 10 ms |
| Refuse a target with at least one incoming reference | Database-operation p95 <= 10 ms and return exactly one bounded witness |

Run each delete case at 100k and 1M rows. The 1M p95 must be less than twice the
100k p95; otherwise the path is scaling like a scan even if it happens to fit on
the reference machine. Also measure write-conflict rate under concurrent Link
churn and delete attempts. No budget increase is accepted merely because the
feature is correct: failure means the implementation remains behind the sync
gate or the database primitive is rejected.

## Staged adoption and implementation exit criteria

1. **Pin the primitive.** Against exactly SurrealDB 3.2.3, prove create,
   retarget, unset, source delete, target delete, self-link, concurrent
   link/delete, explicit transaction rollback, and stable error translation.
   Prove how `REFERENCE` is built for existing rows and whether bulk import
   maintains it. A failed or ambiguous case stops the rollout.
2. **Extend metadata and compilation.** Add optional `on_delete` only to Link
   fields. Omission and `refuse` compile identically. `cascade`, `nullify`, and
   unknown values fail schema sync with a typed unsupported-policy error. The
   DDL remains in the existing runtime-metadata resource path
   (`frust-kernel/orm/src/resource.rs:1-26`); schema drift already snapshots
   fields, indexes, and events (`frust-kernel/orm/src/drift.rs:128-143`).
3. **Activate safely.** Before enabling enforcement on populated data, run a
   resumable integrity audit and reference build. Sync must not publish the new
   metadata generation until every existing non-empty Link is represented.
   Dangling pre-existing Links are reported by source DocType, field, and row;
   they are never silently nulled. Use the migration system's existing schema
   lock, not a new request-path lock.
4. **Expose the typed refusal.** Map only the proven stable database signal to
   `409 link-exists`. Preserve all unknown database errors as errors; broad
   string matching would turn vendor changes into false policy decisions.
5. **Gate and observe.** Pass the cost budget, concurrency test, restart/drift
   test, and an HTTP proof that the target survives and no success realtime tick
   is emitted. The existing delete proof already establishes caller-session
   authorization and successful-delete behavior
   (`frust-kernel/kernel/tests/delete_door.rs:170-228`).

A future build is complete only when all five stages pass on fresh and
pre-populated databases, the generated schema can be reconstructed after
restart, and direct normal-session writes cannot create a delete race. A design
or happy-path HTTP test alone is not an implementation exit.

## Explicit non-goals

- Dynamic Links whose target DocType is stored in another field.
- `Table`/embedded child values, arrays of records, graph `RELATE` edges, and
  record ids in schemaless framework tables.
- Cascade, nullify, force-delete, delete hooks, soft delete, undelete, and bulk
  delete guarantees.
- Preventing a write from linking to a target that never existed. This design
  closes deletion-created dangling references; existence-on-write needs its own
  costed assertion.
- Cross-database or cross-tenant references, which SurrealDB record ids do not
  make into a supported Frust relationship.
- Repairing historical dangling links automatically. Activation reports them
  and refuses to guess.
- Protecting against a database owner who removes or alters the generated
  schema. Direct database administration remains outside the supported door
  (`frust-kernel/docs/evolution-policy.md:76-83`).
