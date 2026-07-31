---
tags: [frust, adr, wasm, plugins, api]
status: accepted
decided: 2026-07-24
---

# ADR-006: The WIT Capability Surface — Structured Verbs, No Strings

**Context:** [[ADR-005 Plugin Isolation]] deliberately excluded the plugin API proper. Opening position (structured filter contract shared by Desk/REST/plugins) survived a six-point grill; four underspecified edges are decided below. This IS the plugin API — everything a plugin can ever do is a verb here.

## Core Decision

One structured contract, three consumers (Desk, REST, plugins). Filters are a structured type — **never a raw query string, anywhere, including the edges**. Consequences that fall out for free: one permission compiler (REQ-3.1.2 applies to plugins automatically), one home for the index policy from [[2026-07-23 SurrealDB week-1 benchmark]], one test surface.

## The Verbs (v1)

| Verb | Shape | Notes |
|---|---|---|
| `db-read` | `(doctype, filters, fields: list<path-segment>, opts) -> result<list<doc>, db-error>` | field-level perms filter the envelope |
| `db-write` | `(op, doc) -> result<doc, db-error>` | hooks always fire — see recursion rule |
| `db-aggregate` | `(doctype, filters, group-by, metrics) -> result<list<row>, db-error>` | `count\|sum\|min\|max\|avg` only; no expressions, no having, no nesting |
| `db-named-query` | `(name, params) -> result<list<doc>, db-error>` | governed escape hatch — see traversal |
| `enqueue` | `(job, payload) -> result<job-id, error>` | identity captured, authority re-derived |
| `log` | `(level, message)` | from the spike |

## The Four Grilled Edges — Decided

> [!note] Edge-1 evidence (WO-019 door probe, 2026-07-26)
> The load-link-time failure mode fired **unprompted**: a demo component importing `db-api` was refused by the hook host at link time with a named missing-instance error — exactly the "never at runtime" promise. Also proven at the same probe: the contract's structural property upgraded from "no raw query strings" to **"query text is un-representable"** — hostile SurrealQL failed identifier validation and filter-parsing, not a denylist. And the one-compiler property is now asserted as an *equality* (route-read == broker-read for the same caller), not an intention.

### 1. Evolution policy: additive-only + two-major host support
The filter/verb contract lives in a versioned `frust:db` WIT world. Operator growth (`contains-any`, `matches`, `within`, …) is **additive variants only**. The host links **two majors** side by side; removals require a major + one-major deprecation notice. Incompatible plugins fail **at load-link time**, never at runtime.

### 2. Traversal: structured paths in, composition out
`fields` takes `path-segment = field(name) | link-hop(field) | edge(direction, edge-type)` with a host-enforced depth cap (default 3, per-DocType configurable). Deep/recursive traversal (BOM explosion, org-chart walk) is **deliberately outside** `db-read`: it ships as **named queries** — authored host-side in metadata, permission-checked, versioned, invoked by name. Plugins *invoke* the graph superpower; they never *compose* it. This is the clause [[ADR-002 SurrealDB Lock-In]] exists for: SurrealQL stays behind the contract.

> [!note] Wire-encoding amendment (2026-07-24, WO-005 module 4)
> WIT has no recursive types: nested list/object values ride the wire as a `compound-v(string)` JSON variant. **Every scalar — including `decimal` — stays first-class**; only *nesting* is JSON-encoded at the boundary. Proven: `decimal_stays_decimal_across_the_boundary` (a `"19.99"` decimal survives the JS engine round-trip as a decimal). Encoding note only; ADR semantics unchanged.

### 3. Doc encoding: typed envelope, dynamic payload — and decimal is in
`doc = list<tuple<string, value>>`; `value` is a variant tree: `null | bool | int | float | decimal(string) | text | datetime | duration | record-id | list | object`. **Plugins get no compile-time field checking** — stated openly; the typed layer is the verbs and hook signatures (where P-2.2 died), not the fields.
**`decimal` is first-class from v1: money never crosses the boundary as a float.** Settles the encoding half of P-3.4; the arithmetic requirement remains an SRS gap-fill.

### 4. Enqueue: *identity is captured, authority is re-derived*
Jobs carry **who**, never a snapshot of what-they-may-do. Permissions re-evaluate at run time (REQ-5.1.2's letter) — revocation must mean revoked; replay of stale authority is the worse bug class. Permission-denied at run = typed, **non-retryable** failure. Payload re-validates against current DocType metadata at run (schema may have migrated since enqueue).

### 5. Aggregation: in v1 (position reversed by the grill)
Absence is the bigger hole: denied aggregation → plugins `db-read` full rows and sum in-sandbox → more DB work, more WASM work, and full row data crossing into the plugin where one permission-checked number should have. The broker centrally applies the benchmark rules: `WITH NOINDEX` for range+sort shapes, attached `TIMEOUT`, epoch budget charged to the calling hook.

### 6. Hook recursion (the correctness hole): cycle-trap + depth cap, no hook-free writes
Every `db-write` from inside a hook carries its **hook chain**. Re-entering a **`(record-id, hook-class)`** already on the chain **traps immediately** — deterministic typed error to the offending plugin, not silent epoch burn across plugins. (Keyed on record, not doctype: Invoice A's hook writing Invoice B is legitimate fan-out; A→…→A is the cycle.) Unbounded *record* chains (A→B→C→…) are caught by the global depth cap **8** as backstop. **Hook-free writes are rejected**: that is Frappe's `db_set` bypass culture (P-3.3, P-5.4) reborn; invariants live in hooks, so hooks always fire. (Audit doesn't depend on this — changefeeds are unbypassable regardless, [[ADR-002 SurrealDB Lock-In|ADR-002]]/[[SurrealDB|§7]].)

## Rejected

- Raw query strings in any parameter (incl. dotted-path strings in `fields`) — a grammar through the back door, ungoverned and unlockable.
- Aggregation expressions / having / nesting in v1 — hold the line at closed metrics.
- Permission-snapshot-at-enqueue — replay risk.
- Hook-free write escape hatch — see #6.

**Related:** [[Frust Hub]] · [[ADR-005 Plugin Isolation]] · [[WASM Component Model]] · [[SRS]] · [[2026-07-24 WASM isolation spike]]
