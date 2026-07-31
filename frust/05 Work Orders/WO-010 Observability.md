---
tags: [frust, work-order, observability]
status: COMPLETED 2026-07-25 — all 5 criteria; one trace reconstructed from logs alone across REST→broker→both runtimes→EVENT reject→enqueue→job; /metrics Prometheus; per-tenant exactly-6/exactly-2 proof; floor held +0–2 ms with tracing on; substrate-probe caveat added; P-8.2 position delivered. → [[2026-07-25 WO-010 Observability]]
created: 2026-07-25
---

# WO-010: Observability (REQ-6.4 — the Last Unimplemented Requirement Family)

> [!info] PM work order — results to `04 Build Log/`, live vault path verified first. This is P-8.2's prerequisite: you cannot throttle what you cannot attribute (REQ-6.4.3 before any quota design).

## Scope

REQ-6.4.1/6.4.2/6.4.3, in the kernel. The `log` verb's "pending tracing" note from module 1 finally comes due.

## Exit Criteria

1. **Trace IDs end-to-end (REQ-6.4.1):** one trace ID born at the REST edge (or `enqueue`) propagates through broker verbs → hook dispatch (both runtimes — the plugin/script `log` verb records it) → worker job runs (the job record carries the originating trace) → DB call sites. Structured (JSON-lines) logs throughout; no bare `println!` survives (grep-gate it like the surql monopoly). Prove: one submit produces a single trace's spans across REST, broker, both hooks, EVENT rejection path, and a follow-on job — reconstructed from the log alone.
2. **Metrics endpoint (REQ-6.4.2):** `/metrics` (Prometheus text format — boring, standard) with: per-verb latency histograms, hook timings by runtime, queue depth + claim-attempt counter (the module-2 retry counter finally exported), worker lag (the WO-007 cursor number), boot/meta version info. No shell access needed to answer "is it healthy."
3. **Per-tenant attribution (REQ-6.4.3):** every span/metric carries the tenant (database) label; a burst against two tenant DBs produces separably-summable per-tenant query time, hook fuel/time, and job time. **The deliverable is the P-8.2 measurement substrate** — end the log with a short position: what quota/throttle designs these numbers can and cannot support (the P-8.2 WO gets written from it).
4. **Typed errors keep their codes in logs:** the error taxonomy (`FRUST:E_*`, `E_IDENTITY_UNRESOLVED`, conflict-exhaustion) appears as machine-readable fields — grep-able incident forensics, per the house style: silent is the enemy.
5. **The floor holds:** 25 ms release gate green with tracing on (sampling/level design is yours; the gate is the judge). Report the overhead number honestly either way.

## Boundaries

- No OTel collector/agent sidecar — the two-process deployment is a load-bearing property (P-2.3). Tracing lives in the kernel's own structured output; export formats can come later if ever needed.
- Log verbosity is config, but the *default* posture answers an incident (the WO-002/WO-008 debugging sessions are the benchmark: could those have been solved from logs alone?).

## Escalations

Standard rules — including the churn-restart caveat before trusting any perf A/B.

**Related:** [[Frust Hub]] · [[SRS]] (REQ-6.4) · [[ADR-003 Tenancy Model]] (P-8.2 open) · [[2026-07-24 WO-007 aggregates ladder implementation]] (lag) · [[2026-07-24 Module 5 close — worker loop]] (claim counter)
