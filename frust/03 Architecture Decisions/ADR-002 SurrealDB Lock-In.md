---
tags: [frust, adr, surrealdb]
status: accepted
decided: 2026-07-23
---

# ADR-002: Accept SurrealDB Lock-In — Portability Lives at the App Contract

**Context:** [[SurrealDB#Risks & Reality Checks]] flagged lock-in as accepted-but-unwritten. Deep use of PERMISSIONS, EVENTS, LIVE SELECT, graph traversal, and changefeeds means no realistic DB swap later.

**Decision:** Accept the lock-in deliberately. **Portability lives at the app contract** — DocType metadata + record JSON + the REST/gRPC API surface — not at the storage layer. Apps and UI never see SurrealQL.

**Rejected:** a database abstraction layer. Lowest-common-denominator adapters would forfeit every modality win in [[SurrealDB#Modality → Requirement Map]] — the modalities *are* the architecture. An abstraction that can't express `LIVE SELECT` or in-engine PERMISSIONS is a tax with no payout.

**Evidence:** [[2026-07-23 SurrealDB week-1 benchmark]] — engine handles every Frappe report shape at interactive speed; the bet is sound enough to commit to.

**Watch-items (re-open this ADR if one fires):**
- **License** — BSL 1.1; re-check if the business model drifts toward hosted-DB offerings.
- **Upgrade churn** — 3.x already renamed `type::thing` → `type::record`; dialect stability is not guaranteed.
- **Planner quality** — [surrealdb#7432](https://github.com/surrealdb/surrealdb/issues/7432); mitigated by REQ-1.2.1 index hints, but a pattern of planner regressions would change the calculus.
- **⬆ PROMOTED (2026-07-24): silent-misbehavior pattern — two instances at v3.2.0.** #7432 (planner silently mis-uses range indexes) + [#7433](https://github.com/surrealdb/surrealdb/issues/7433) (changefeed datetime-`SINCE` silently returns empty against a populated feed — confirmed with airtight UTC timeline, [[2026-07-24 Architecture skeleton (WO-002)]]). The pattern is *silent wrong answers, not errors*. Standing mitigation: Frust code uses versionstamp-`SINCE` only (datetime form banned); every SurrealDB feature gets an empirical first-exercise before any code trusts it (the WO-002 criterion-3 discipline, now permanent). A third instance triggers a formal re-read of this ADR.

**Related:** [[Frust Hub]] · [[SurrealDB]] · [[SRS]]
