---
tags: [frust, work-order, desk, topcoat]
status: COMPLETED 2026-07-25 — all 6 criteria met, acceptance flow clicked in a real browser; X-Frust-Role deleted (sessions = records, restart-surviving); Tier-0 rules are code (raw ranges inexpressible); 11.8–14.1 ms screens vs the 620 ms line; 2 broker hardenings + decimal-string script discipline finding. → [[2026-07-25 WO-009 Desk v1]]
created: 2026-07-24
---

# WO-009: Desk v1 — The Product's Face on the Hardened Kernel

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. Governing contracts: [[ADR-004 Topcoat for Desk v0]] (headless: `(metadata, record JSON) → render`), [[ADR-001 UI Extension Tiers]] (Tier-1 metadata rendering), [[ADR-010 Materialized Aggregates]] (Tier-0 rules live here).

## Scope

`frust-proto` graduates from proof-harness to Desk v1: the metadata-driven UI a Frappe user would recognize, speaking only the kernel's REST surface.

## Exit Criteria

1. **Session-token auth — the WO-008 residue dies first.** Login → kernel-issued session; REST derives *both* permission halves (row + field envelope) from the session principal. **`X-Frust-Role` is deleted.** A tampered/absent role claim changes nothing the DB doesn't already enforce.
2. **Metadata-driven list + form for any DocType** — rendered from `doctype` metadata at request time (the WO-002 prototype's property, now against live kernel metadata): list view with filters/sort/paging through the filter contract, form view with typed fields (decimal rendered as decimal), submit through the full hook chain, typed errors (`E_DOCSTATUS`, `HookRejected`, `E_IDENTITY_UNRESOLVED`) rendered as user-facing messages — engine internals stripped per ADR-007's hygiene rule.
3. **Tier-0 shape rules, implemented where ADR-010 assigned them:** list/report queries apply period-bucketing (`month = 'YYYY-MM'` pickers, never raw date ranges on big tables) and entity-equality shapes. The rule table from the WO-007 log is the spec.
4. **Rollup-backed reports:** monthly revenue and AR outstanding pages read Tier-1 rollup DocTypes (16–51 ms class, not 7.7 s class); item-wise reads Tier-2 rollups **with the lag indicator visible** (cursor staleness as data — dashboards show staleness rather than hiding it, per ADR-010).
5. **The docstatus lifecycle in the UI:** draft → submit → cancel with state-appropriate affordances (read-only at 1 except allowlisted fields, no edits at 2), driven by metadata, enforced by the lattice EVENT underneath — the UI *reflects* the floor, never reimplements it.
6. **The WO-002 acceptance flow, clicked, not curled:** create a DocType in the UI, its list/form appear without restart, submit as clerk/manager, see the audit trail — the exit sentence as a user experience.

## Boundaries (standing)

- Topcoat pinned rev; roadmap-watch buckets apply — no Topcoat auth/jobs/ORM ([[Topcoat#Upstream Roadmap Watch (reviewed 2026-07-24)|🚫 bucket]]).
- Realtime list updates stay **polling** (REQ-6.5.2's degradation path) until Topcoat ships a push transport — no bespoke socket layer.
- Spreadsheet-grade grid editing is explicitly out of scope (ADR-004 revisit-trigger #1 stays armed, not tripped).

## Escalations

Standard rules. Slow-4G interaction budget: the 620 ms shard round-trip number governs — dependent-field logic stays client-side (ADR-007 client half); if a v1 screen can't hold that line, report the screen and the number.

**Related:** [[Frust Hub]] · [[ADR-004 Topcoat for Desk v0]] · [[ADR-007 Tier-2 Script Architecture]] · [[ADR-010 Materialized Aggregates]] · [[2026-07-24 WO-008 identity hardening]]
