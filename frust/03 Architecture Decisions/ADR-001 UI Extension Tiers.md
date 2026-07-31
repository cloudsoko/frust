---
tags: [frust, adr]
status: accepted
decided: 2026-07-23
---

# ADR-001: Tiered UI Extension Model

> [!success] ALL THREE TIERS SHIPPED (2026-07-26, WO-017 close)
> Tier 1 metadata (WO-002/005/009), Tier 1 behavior (WO-014), Tier 2 sandboxed scripts both hosts (WO-001/005 server-side principle; WO-017 browser: lazy-load, worker+watchdog containment, decimal catch, Desk authoring), Tier 3 compiled plugins (ADR-005/006). The day-one phasing decision ran to completion: metadata first, sandbox when built, marketplace path intact. Server-side per-DocType script *delivery* rides [[WO-019 App Lifecycle]].

**Context:** [[Topcoat]] (and compile-time Rust UI generally) has no dynamic code-loading path — views are proc-macro expanded, `topcoat ui add` vendors source. Meanwhile [[Frappe Pain Points|Frappe]]'s user-authored client scripts are a major adoption feature we refuse to forecline. Serves [[SRS#2.2 Event Hooks & Extension Points|REQ-2.2.3]], addresses P-7.5.

**Decision:** Three tiers — end state is tiered; **only Tier 1 ships first**.

| Tier | What | When | Mechanism |
|---|---|---|---|
| 1 | Metadata: fields, layouts, configs, views | Runtime, **v0** | Generic components walk DocType metadata; forms render for DocTypes that didn't exist at compile time |
| 2 | User-authored client scripts | Runtime, later | Sandbox attached to Tier-1 hook points; no DOM access — a bridge of ~6 verbs (get/set value, toggle visibility/read-only, validate, call server method) → signal writes + shard/procedure calls |
| 3 | Novel compiled widgets | Recompile | Curated marketplace; recompile-and-redeploy acceptable here only |

**Load-bearing detail:** the metadata schema defines named lifecycle hook points (`on_load`, `on_change`, `validate`, …) **from v0**, before any sandbox exists. Hook points cost ~nothing now; retrofitting them into metadata already living in production [[SurrealDB]] documents is the expensive version.

**Consequences:**
- v0 needs no sandbox engine — the riskiest component is deferred until a form has rendered.
- The sandbox tier is much smaller than "embed a browser scripting environment": a 6-verb bridge, not DOM access.
- Rejected: metadata-only-forever (bakes loss of user scripts into the spec); sandbox-from-day-one (front-loads maximum risk, discards Rust's free compiled-extension path).

> [!success] Tier 1 extended to *behavior* (WO-014, 2026-07-25)
> `depends_on` / `read_only_when` / `required_when` / validate / `fetch_from` as declarative field metadata, compiled at render time to per-field signals + bridge verbs — client dynamics with **no recompile, no restart, zero round-trips** (2-request network log). The lattice outranks the dynamism; money rules compare-never-compute (type-enforced). Remaining Tier-2 piece: untrusted script *text* in-browser (`script-engine.wasm` against the six verbs). [[2026-07-25 WO-014 Desk v2 dynamic forms]]

> [!success] Tier 2 realized (2026-07-24)
> The sandbox tier is now decided: [[ADR-007 Tier-2 Script Architecture]] — a *profile* of the plugin surface (one JS dialect, two hosts, zero bespoke sandboxes), not a second system. The "may converge on the same runtime" hope from [[WASM Component Model]] came true.

> [!note] ~~Constraint from prototype (2026-07-23)~~ — **AMENDED 2026-07-25, constraint lifted**
> Original finding: signals were compile-time items → the bridge must compile to generic shard re-renders (~15 ms per driver change). **Premise now false:** dynamic signals (vendored Topcoat work, [[2026-07-25 Topcoat vendored - 175 signal methods]]) enable per-metadata-field signals in a runtime loop. The six-verb bridge is built on them ([[2026-07-25 Tier-2 six-verb form bridge]]): **five of six verbs run at zero round-trips** (only `call-server` touches the network — proven by browser network log). Strictly better than this ADR assumed; the shard re-render remains the fallback for logic that genuinely needs server state. Money boundary held structurally: the Decimal surrogate has no client arithmetic — stored-value computation is impossible by construction.

**Related:** [[Frust Hub]] · [[Topcoat#Prototype Exit Criteria — ✅ all passed 2026-07-23]] · [[2026-07-23 Topcoat prototype]]
