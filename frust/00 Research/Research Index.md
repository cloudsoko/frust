---
tags: [frust, research, competitive-intel, moc, vision]
status: index — maintained as reports are added
created: 2026-07-30
---

# Research Index — Competitive & Predecessor ERP Intelligence

> [!info] What this folder is
> External deep-research reports on the ERP ecosystems Frust competes with or descends from, imported to **ground the vision in evidence**. The Frappe report is corroboration and expansion of [[Frappe Pain Points]] — the P-x.x list that *is* Frust's reason to exist. The Odoo report is intel on the other major open-source ERP, and most of its structural constraints are ceilings Frust has already **measured** against. These are reference material, not work orders: they feed [[Frappe Pain Points]], [[SRS]], and the [[v1.0 Pain-Point Scorecard]]; they don't gate builds.
>
> *Provenance note:* the reports carry inline `citeturn…` tokens — source-citation artifacts from the research tool, left intact as evidence markers. Snapshots are late-July 2026.

## The reports

- [[frappe-research-report]] — **Frappe ecosystem.** Thesis: no single fatal flaw, but an *accumulation of platform-friction* — upgrade/version coordination across 217 repos + split apps, security-that-depends-on-rapid-upgrades, list/report perf at scale, Bench/Docker/Helm deployment complexity, an unresolved OpenAPI/API-v2 gap, and uneven app-lifecycle maturity. The bottleneck is *ecosystem coordination*, not raw framework capability.
- [[odoo-research-report]] — **Odoo 19 ecosystem.** Thesis: a powerful, opinionated integrated suite whose strengths (shared model, modular stack) and weaknesses (heavy ORM coupling, GIL-bound default server, no first-party queue, upgrade/customization coupling, large-table valuation slowdowns, edition/hosting-gated extensibility) come from the *same* design choices. Success is governed by customization discipline, not the product alone.

## Findings → Frust (why these reports matter to us)

Each row: a research finding → Frust's answer and the WO/ADR that delivered it. Exact killed/bounded/open verdicts live in [[v1.0 Pain-Point Scorecard]].

### Frappe report → Frust

| Frappe-ecosystem finding | Frust's answer (WO/ADR) |
|---|---|
| No native OpenAPI; API v2 unstable; discoverability weak | SurrealDB speaks REST/GraphQL natively; kernel = **one permission compiler × three byte-equal consumers** (broker/REST/Desk) — Frust doesn't *generate* an API layer, the store does |
| Security posture depends on rapid upgrades; permission-bypass advisories | **DB-enforced** row permissions (compiled once, byte-equal); P-5.x security clean sweep; fail-closed boot key-guard ([[ADR-013 Signing-Key Integrity at Boot]]) |
| List/report performance degrades at scale | 1 M-row release-mode floor holds (~25 ms); index-hints mandatory (30× range-index trap banked in [[SurrealDB]]); [[ADR-010 Materialized Aggregates]] ladder |
| Deployment complexity (Bench/Docker/Helm, *nix-only, bench-shared config) | **Two-process deployment** (`frust serve` + `surreal.exe`); [[DR Runbook]]; ~25 s accepting-boot budgeted |
| Vertical-first scalability, no clear horizontal path | WO-025 concurrent loop **15→124 req/s** (arc through WO-026); P-1.1 killed; 60–76 MB footprint (P-1.4 killed) |
| Split-app upgrade/migration breakage | Installed app (DocType+hook+rollup+data) **survives the two-step major upgrade functional, hook still fires** (WO-036 A2) |
| Bench-shared multi-tenancy (all sites share bench config) | WO-040 **runtime-selected tenancy**, 4 topologies behind one guarded seam, per-tenant restore, isolation-by-provenance; P-8.1 killed |
| jQuery/Vue stack heterogeneity; extension complexity | **A TRADE, not a solve** (reframed 2026-07-31 — the original row was too triumphalist): Frust *sidesteps* the heterogeneity pain by not offering the client-side app-building tier that caused it (headless kernel [[ADR-004 Topcoat for Desk v0]], one SSR Desk, [[ADR-001 UI Extension Tiers]]). The trade is priced in [[ADR-016 Frontend Posture]] (proposed): BYO-frontend supported against the REST surface; SSR-native kit = named M5 candidate coupled to topcoat #275. |

### Odoo report → Frust

| Odoo-ecosystem finding | Frust's answer (WO/ADR) |
|---|---|
| Default server GIL-bound; multiprocessing steered, Windows excluded | No Python, no GIL — the **single-thread accept loop was the *new* ceiling**, killed by the WO-025 worker pool (P-1.1) |
| No first-party async queue (ecosystem leans on OCA `queue_job`) | Native **table-as-queue** ([[ADR-009 Execution Model]]), 1.01 attempts/claim, Tier-2 worker rollups ([[ADR-010 Materialized Aggregates]]) |
| Raw SQL bypasses access rules; `sudo()` crosses company boundaries | Permissions are **DB-enforced with no bypass surface**; a caller **cannot address another tenant's DB** (JWT ns/db pins the session; forged → 401), WO-040 |
| 20-min valuation reports / MRP degradation on large tables | Rollup ladder ~**275× monthly** (16–51 ms vs 7.7 s); 1 M-row floor 25 ms |
| Multi-company correctness risk (users logged into many at once) | Per-request token split **before any DB call**; provenance-asserted isolation (WO-040 exit proof: 0 foreign rows) |
| Accounting: localization gaps, journal imbalance, upgrade defects | **Decimal end-to-end, compare-never-compute**, mul/div explicit half-even (WO-021/030); DB↔Rust byte-equal as a CI property |
| Each API call its own transaction; RPC removal churn; no GraphQL | SurrealDB native protocols; structured-verb capability surface ([[ADR-006 Plugin Capability Surface]]) |
| Odoo Online forbids custom code; no-code ceiling | [[ADR-001 UI Extension Tiers]] all shipped: plugin routes (WO-019), server scripts, hostile-contained in-browser sandbox (WO-017) |

## What Frust should still watch (honest gaps the reports surface)

- **Batteries.** Both ecosystems' depth lives in the "last 20%" (Frappe: payroll, Helpdesk i18n; Odoo: localization breadth). Frust's turnkey half is only starting — [[WO-043 Email Batteries]] is the first; print/PDF is an unstarted spike.
- **Cross-app orchestration / extension model.** Frappe's CRM↔ERPNext seam and Odoo's edition-gated extensibility are both product blockers; Frust's cross-app extension model (strict one-owner vs declared extension points) is an open M4 design question.
- **DB parity / scale beyond one node.** Frappe's Postgres-is-beta and Odoo's large-table valuation both show single-engine scaling limits; Frust's own bound is ~150 w/s (architecture: batching/sharding/per-tenant process), stated in [[v1.0 Pain-Point Scorecard]].

## Related

[[Frust Hub]] · [[Frappe Pain Points]] · [[SRS]] · [[v1.0 Pain-Point Scorecard]] · [[v2.0 Deployability Gate]]
