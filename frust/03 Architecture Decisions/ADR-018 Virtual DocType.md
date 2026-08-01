---
tags: [frust, adr, virtual-doctype, integration, permissions, proposed]
status: PROPOSED
decided: 2026-08-01
supersedes: none
---

# ADR-018: Virtual DocType — Frust Does NOT Do Live Proxies; It Syncs to a Real Table

> [!warning] STATUS: **PROPOSED** — do not treat as ratified.
> This is the WO-062 probe's position paper. The verdict below is a **STOP** on the obvious feature and a redirect to a different one; the Boss ratifies or vetoes. Nothing was shipped — the probe was scratch-only (`zero shipped kernel edits`, WO-022 rule). The `is_virtual` flag does **not** exist in the codebase and must not until this is accepted.

## Context

Frappe's **Virtual DocType** (`is_virtual`) has fields and the full document API but **no table**: data comes from an external source (another DB, a REST API) through controller data-access methods. It is Frappe's **integration primitive** — the way you wrap an external logistics / payment / CRM system behind the DocType interface and get lists, forms and permissions "for free". It is strategically attractive for the M5 ecosystem theme and the Fleetbase-style SaaS-consumer context.

But it is in **direct tension with Frust's alpha**, and WO-062 was scoped as a **probe → ADR, not a build**, precisely because that tension might be fatal.

**The alpha (why this is not an ordinary feature).** Frust's differentiator is that row/field permissions are enforced **by the database** — a SurrealDB `PERMISSIONS` clause, evaluated **under the caller's own session token**, compiled **once** and served byte-equal to REST, Desk and plugins ([[ADR-006 Plugin Capability Surface]] edge-1: *route-read == broker-read*). The kernel cannot over-read even by bug. A **Single** ([[WO-061 Single DocType]]) keeps this trivially — its data is one row in a real table. **Virtual's data is not in the database at all**, so the database cannot run `PERMISSIONS` on it. That is the crux.

## The probe (WO-062) — predictions stated first (WO-019 template)

| Gate | Prediction | Outcome |
|---|---|---|
| 1 — permissions on non-DB data | No structural (DB-enforced) shape exists; only bypassable in-connector app-code filtering → **STOP** | **CONFIRMED — STOP** |
| 2 — capability / containment | Outbound-HTTP is absent from the sandbox (ADR-006) *and* gated inside SurrealDB itself; adding it is an amendment | **CONFIRMED** |
| 3 — request path | A live external fetch blocks a worker and fails the ~25 ms floor (REQ-6.1.1) | **CONFIRMED (by construction + WO-024/038 lesson)** |
| 4 — honest limits | Live-proxy silently loses changefeed audit, LIVE realtime, and the typed-decimal/hook envelope | **CONFIRMED** |
| 5 — the alternative | Sync-to-real-table preserves the whole alpha at the cost of staleness + storage | **CONFIRMED — RECOMMEND** |

### Evidence (SurrealDB 3.2.0, scratch instance, root/root)

- **G1a — the DB has no per-caller external-fetch primitive.** `RETURN http::get('http://…')` is refused: `kind: NotAllowed, "Access to network target … is not allowed"`. SurrealDB gates *all* outbound HTTP behind an explicit server capability (`--allow-net`), and when enabled it runs **under the database server process, not the caller's session** — so it can never be the mechanism that makes an external read respect *this user's* permissions. The DB itself treats network as a boundary to be granted, which is corroboration for gate 2, not just gate 1.
- **G1b — a function's returned rows carry no permission surface.** `DEFINE FUNCTION fn::ext_rows() { RETURN [{owner:'clerk1',secret:'A'},{owner:'manager',secret:'B'}] }` returns **both rows verbatim** to any caller. There is no `PERMISSIONS FOR select` on a function result, on a computed value, or on anything that is not a table row. The *only* way to make `fn::ext_rows()` role-aware is to write `IF $auth.role = …` **inside the body** — hand-authored, per-connector, un-compiled, and bypassable by any other code path that reads the same source. **That is exactly the STOP condition: it breaks the one-compiler alpha.**
- **G5 — a real table keeps everything.** `INFO FOR TABLE company_settings` (the WO-061 single) shows the ordinary `PERMISSIONS` and `CHANGEFEED` a synced DocType gets. A table that a worker *mirrors* external data into is a normal DocType: permissions compile once, the changefeed is the unbypassable audit trail (ADR-002 §7), and LIVE works (WO-011/012). The alpha holds unchanged.

### Frappe-source verification (PM, 2026-08-01 — the premise checked against the real code, not memory)

Confirmed in the actual Frappe 16.14 source (`D:\Dev\Soukify\frappe`):
- **The interface** (`frappe/model/virtual_doctype.py`): a virtual doctype is a `Document` controller where the **developer implements ALL data access** — static `get_list`/`get_count`/`get_stats` + instance `db_insert`/`load_from_db`/`db_update`/`delete` — against, verbatim, *"any storage service … a database, flat file or **network call to API**."* No table; Frappe stores nothing.
- **The permission crux** (`frappe/model/db_query.py:207`): `DatabaseQuery.execute()` — the method that normally applies Frappe's `permission_query_conditions` / role / user-permission filtering as SQL — **short-circuits for a virtual doctype**: `if is_virtual_doctype(...): controller = get_controller(...); ... return controller.get_list(**kwargs)`. Frappe's framework row-level permission machinery is **bypassed**; row-level enforcement is entirely the developer's `get_list` code. (Coarse doctype+role `has_permission` still runs in Python; row-level does not.)

**This validates the STOP, and sharpens *why*.** Frappe's virtual doctype works *for Frappe* precisely because Frappe **never had DB-enforced permissions** — its whole model is app-layer Python (that is pain point **P-5.3**, "leaky permissions / `permission_query_conditions` string-concat SQL"). A virtual doctype fits Frappe because it's app-code-permissions all the way down anyway. **Frust's alpha is the opposite** — permissions compiled once and enforced *by the database*. So a Frust live-virtual-DocType wouldn't be "matching Frappe" — it would be **abandoning the exact guarantee that differentiates Frust from Frappe**, to adopt the model of the pain point Frust exists to kill. The sync-to-real-table choice is therefore not a capability gap; it's a refusal to trade the alpha for parity with a Frappe weakness.

## Decision

**Frust does NOT support live virtual DocTypes. Frust integrates external systems by SYNC-TO-REAL-TABLE (the consumer / mirror pattern).**

A metadata-configured background worker (the proven [[ADR-010 Aggregate Ladder|Tier-2 worker]] / [[2026-07-30 WO-043 mail transport decision|WO-043 mail worker]] posture — `std::thread`, off the request path) fetches an external source on an interval and **writes it into a real DocType**. From that point on, every consumer reads a normal table:

- **Permissions** — DB-enforced, one compiler, byte-equal across REST/Desk/plugins. The alpha, untouched.
- **Audit** — the changefeed records every mirror write (REQ-3.2.1). "When did this external record change in our system" is answerable.
- **Realtime** — LIVE / SSE work, because there is a changefeed to watch (WO-011/012/032).
- **Decimal safety** — external money passes through the typed envelope on the way in (WO-016/030); a `Currency` field is `decimal`, not float.
- **The floor holds** — reads never make a network call (WO-024/038); the request path touches only SurrealDB.

**The price, stated plainly:** *staleness* (bounded by the sync interval) and *storage* (a copy of the mirrored subset). For an ERP consuming external logistics / payment / CRM data, staleness is tunable and usually acceptable, and storage is cheap. The environment's own Fleetbase guidance leans the same way — **consume/sync, don't live-mirror**.

## Rulings

1. **Gate 1 (load-bearing) — STOP on live-proxy.** There is no structural way to enforce Frust's row/field permissions on data the database does not hold. In-connector `IF $auth…` filtering is un-compiled, per-connector and bypassable — it re-opens P-2.2/P-5.x and abandons the one-compiler guarantee that is the product's reason to exist. **A live virtual DocType would trade the alpha for a convenience. Refused.**
2. **Gate 2 — outbound-HTTP is a contained *worker* concern, not a sandbox verb.** The mirror fetcher's network access lives in the kernel worker (like the WO-043 mail relay), contained (host allowlist, timeouts, no SSRF to internal/loopback), and **off the per-request surface**. The ADR-006 capability table is *not* widened. **If** app-authored (WASM) connectors are ever wanted — so a bundle can define its own source — that requires an **outbound-HTTP capability = an ADR-006 amendment (a security boundary; profile tables are boundaries)**, which is **proposed here, not ratified** (see below). It is explicitly *not* needed for the recommended shape.
3. **Gate 3 — the fetch never sits on the read/write path.** Mirror fetches are worker-driven and asynchronous; the read path is a plain DB read at the ordinary floor. This is the same discipline as ADR-010 rollups and WO-043 mail: *network belongs in a worker that owns its own blocking, never in an `async`/request handler* (the standing async-blocking note).
4. **Gate 4 — no live virtual DocType may pretend to have the audit/realtime it cannot have.** Because the recommendation forbids live-proxy, this becomes a non-issue: the mirror *does* have audit and realtime. Were a live-proxy ever built anyway, its loss of changefeed audit and LIVE **must be surfaced at declaration time**, never discovered — a virtual DocType that silently drops the audit trail is worse than none.
5. **No-recompile is preserved by a GENERIC mirror worker.** One kernel worker, driven by **metadata** (source URL, auth ref, field mapping, interval, target DocType), serves N sources with no per-connector recompile — the metadata-driven thesis intact. A kernel-native connector-per-integration (recompile each) is the fallback for sources too bespoke for the generic mapping, and is a deliberate, small, blessed set — never the default.

## Rejected

- **Live-proxy (Frappe's `is_virtual`)** — sacrifices DB-enforced permissions, changefeed audit, LIVE realtime and the typed-decimal envelope; blocks the request path. The whole reason Frust exists is the set of guarantees this throws away.
- **In-connector permission filtering as "good enough"** — un-compiled, per-connector, bypassable. This is the P-5.4 / `db_set`-bypass culture reborn one layer out.
- **Outbound-HTTP in the WASM sandbox for v1** — not needed for the recommended shape, and a containment-boundary expansion that must not be smuggled in under an integration feature.

## Proposed follow-ups (NOT decided here)

- **A sync-mirror work order** (post-ratification): the generic metadata-driven mirror worker, built on the ADR-010 Tier-2 / WO-043 templates. Not built by WO-062 (probe-only). The mirror table is an ordinary DocType, so it inherits WO-061's proof that the alpha applies to it.
- **[PROPOSED, self-ratification forbidden] An ADR-006 outbound-HTTP amendment** — *only if* app-authored connectors become a requirement. It would add a contained `http-fetch(request) -> result` verb to a **new** `frust:net` WIT world (additive = a new world, per ADR-006's WO-053 refinement), with a host-enforced allowlist, timeout, response-size cap, and a hard SSRF block on internal/loopback/metadata addresses. This is a security-boundary change and belongs to the Boss, not to a builder.

**Related:** [[ADR-006 Plugin Capability Surface]] · [[ADR-002 SurrealDB Lock-In]] · [[ADR-010 Aggregate Ladder]] · [[WO-061 Single DocType]] · [[WO-062 Virtual DocType Probe]] · [[2026-08-01 WO-062 virtual doctype probe]] · [[SRS]] (REQ-3.1.2, REQ-3.2.1)
