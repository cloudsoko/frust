---
tags: [frust, frappe, pain-points]
status: living-document
created: 2026-07-23
---

# Frappe Pain Points — Why Rewrite in Rust

> [!info] Purpose
> The canonical list of problems in Frappe (v15/v16) that **Frust** must solve. Every design decision during the build should trace back to a pain point here and a requirement in [[SRS]]. Add new pain points as we discover them — this is a living document.

## 1. Performance & Concurrency

- **P-1.1 — Python GIL & sync workers.** Gunicorn sync workers, one request per worker. Concurrency = spawning more heavyweight processes. No true async request handling in the core. → [[SRS#1. Dynamic Schema & Data Management System]]
- **P-1.2 — Metadata loaded per request.** DocType meta is deserialized from DB/Redis cache on nearly every request. High baseline latency (~50–200ms for trivial reads).
- **P-1.3 — ORM inefficiency & N+1 queries.** `frappe.get_doc` loads full documents + all child tables even when one field is needed. Hooks trigger cascading fetches. No compiled/prepared query plans.
- **P-1.4 — Memory footprint.** Each worker holds the full framework + all installed apps in memory (~300–500MB/worker). Scaling out is expensive.
- **P-1.5 — Optimistic locking is crude.** `TimestampMismatchError` ("Document has been modified") pushed onto users instead of proper conflict resolution. Frequent under concurrent editing.
- **P-1.6 — Single-threaded scheduler.** One scheduler process ticks all sites; long tick handlers delay everything else.

## 2. Architecture & Runtime

- **P-2.1 — No app isolation.** All apps share one Python process and namespace. A buggy third-party app can crash workers, monkey-patch core, or corrupt shared state. → [[SRS#2. Dynamic Plugin Architecture & Hot Swapping]] (REQ-2.1.2)
- **P-2.2 — Hooks are global mutable magic.** `hooks.py` merging is order-dependent and opaque. Debugging "which app changed this behavior" is archaeology.
- **P-2.3 — Heavy polyglot stack.** Python + MariaDB + Redis (×3 instances) + Node (socket.io) + wkhtmltopdf + bench tooling. Deployment/ops complexity is enormous for what it does.
- **P-2.4 — Realtime is a bolted-on Node sidecar.** socket.io process bridged through Redis pub/sub; fragile, separately deployed, weakly integrated with permissions.
- **P-2.5 — Dependency hell between apps.** Apps share one `requirements.txt` resolution; version conflicts between apps are unsolvable without forking.
- **P-2.6 — Restart-required changes.** Despite being "dynamic", changing hooks, Python controllers, or installing apps requires `bench restart` + `bench migrate`. Only DB-stored customizations are truly live. → [[SRS#2. Dynamic Plugin Architecture & Hot Swapping]] (REQ-2.1.1)

## 3. Type Safety & Correctness

- **P-3.1 — Zero static guarantees.** Everything is `dict`-shaped and stringly-typed. Field renames break silently at runtime, in production, in some hook.
- **P-3.2 — Validation scattered across layers.** Controller `validate()`, client scripts, DB constraints, and meta validation can disagree. Data invariants live in 4 places. → [[SRS#1. Dynamic Schema & Data Management System]] (REQ-1.2.2)
- **P-3.3 — Silent failure culture.** Pervasive `ignore_permissions=True`, `ignore_validate`, bare `except` blocks in core and ecosystem apps.
- **P-3.4 — Float-based money.** Currency stored/computed as floats with rounding patches sprinkled around; precision bugs in accounting are endemic.

## 4. Schema & Migrations

- **P-4.1 — Fragile schema sync.** `bench migrate` DDL is not transactional (MySQL/MariaDB limitation, unhandled). A failed patch mid-migration leaves DB and meta out of sync. → [[SRS#1. Dynamic Schema & Data Management System]] (REQ-1.1.3)
- **P-4.2 — Customizations vs fixtures vs patches.** Three overlapping mechanisms (Custom Field, Property Setter, fixtures, patches) with unclear precedence; sync conflicts on upgrade.
- **P-4.3 — Renames are destructive.** Renaming a DocType/field is a data migration event with cascading breakage across reports, prints, scripts, and links.
- **P-4.4 — No schema versioning/rollback.** Migrations only go forward; no down-migrations, no dry-run diff.

## 5. Security

- **P-5.1 — Server Scripts are sandboxed `eval`.** RestrictedPython sandbox with a history of escapes; arbitrary code stored in the DB.
- **P-5.2 — Raw SQL escape hatch.** `frappe.db.sql` with f-strings is pervasive in the ecosystem; SQL injection depends on developer discipline, not the framework.
- **P-5.3 — Permission model is complex yet leaky.** Role perms + user perms + share + `if_owner` + hooks-based `permission_query_conditions` interact unpredictably; row-level security is opt-in via string-concatenated SQL fragments. → [[SRS#3. Security & Fine-Grained Access Control]]
- **P-5.4 — Audit trail is best-effort.** Version doctype tracking can be bypassed (`db_set`, raw SQL, `flags`), so the audit log is not authoritative. → [[SRS#3. Security & Fine-Grained Access Control]] (REQ-3.2.1)

## 6. Background Jobs & Async

- **P-6.1 — RQ jobs are fire-and-forget.** Weak retry semantics, jobs lost on worker crash, stuck-job debugging is a routine ops task. → [[SRS#5. Background Jobs & Asynchronous Processing]]
- **P-6.2 — No job idempotency or exactly-once support.** Deduplication and locking are DIY (`frappe.utils.locks`), inconsistently applied.
- **P-6.3 — Queue starvation.** Long jobs on the `default` queue block short ones; queue routing is manual and often wrong.

## 7. Developer & Operator Experience

- **P-7.1 — Bench is a house of cards.** Symlinked apps, supervisor configs, nginx templates, `sites/` directory conventions — one wrong command breaks the environment.
- **P-7.2 — Testing is slow and stateful.** Tests hit the real DB, share global state, and the suite takes long enough that people skip it.
- **P-7.3 — Upgrade pain.** Major version upgrades (v13→v14→v15) routinely break custom apps; deprecations land without static detection.
- **P-7.4 — Weak observability.** No structured logging by default, no tracing, no metrics endpoint; debugging production = tailing text logs.
- **P-7.5 — Legacy Desk UI coupling.** Server-rendered + jQuery-era Desk tightly coupled to framework internals; building alternative frontends means reverse-engineering `/api/method` behaviors.

## 8. Multi-tenancy

- **P-8.1 — Site-per-directory tenancy.** Each site = separate DB + config + assets under `sites/`; no shared connection pooling, per-site cold caches, migrations run serially per site.
- **P-8.2 — No tenant resource isolation.** One tenant's heavy report starves every tenant on the bench.

---

## What Frappe Gets Right (don't lose these)

> [!success] The rewrite must preserve these — they're why Frappe wins despite everything above.

- **Metadata-driven everything** — DocType as single source of truth generating DB schema, forms, list views, API, permissions. → [[SRS#1. Dynamic Schema & Data Management System]]
- **Zero-to-CRUD speed** — a new business object with UI + REST API in minutes, no code.
- **Docstatus lifecycle** — draft/submitted/cancelled is a genuinely good fit for ERP documents. → [[SRS#4. Document State & Workflow Machine]]
- **Batteries included** — auth, roles, email, print formats, reports, files, comments, assignments out of the box.
- **Customization without forking** — Custom Fields, Property Setters, client/server scripts layered over core.

## Pain Point → Requirement Coverage

| Pain area | SRS section | Gaps to spec later |
|---|---|---|
| Performance (P-1.x) | implicit | ⚠️ No explicit perf/latency requirements in SRS yet |
| Isolation (P-2.1) | REQ-2.1.2 | ✅ |
| Hot reload (P-2.6) | REQ-1.1.2, REQ-2.1.1 | ✅ |
| Validation (P-3.2) | REQ-1.2.2 | ✅ |
| Money precision (P-3.4) | — | ⚠️ Needs a decimal-arithmetic requirement |
| Migrations (P-4.x) | REQ-1.1.3 | ⚠️ No rollback/dry-run requirement |
| Security (P-5.x) | REQ-3.x | ⚠️ No sandboxing-of-user-scripts requirement |
| Jobs (P-6.x) | REQ-5.x | ⚠️ No retry/idempotency requirement |
| Observability (P-7.4) | — | ⚠️ Missing entirely |
| Multi-tenancy (P-8.x) | — | ⚠️ Missing entirely — decide: single-tenant or multi? |
| Realtime (P-2.4) | — | ⚠️ Missing (websocket/pubsub requirement) |

## Related

- [[Frust Hub]] — project home
- [[SRS]] — requirements specification
