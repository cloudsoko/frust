---
tags: [frust, build-log, kernel, work-order, queue]
created: 2026-07-24
work-order: "[[WO-005 Metadata Kernel v0]]"
---

# Build Log — Module 5 Close: Worker Loop (Exit Criteria 5, 6, 7 EXECUTED)

## Exit criterion 6 — the contested atomic claim, measured

`worker_queue::criterion6_exactly_one_winner_per_job_under_burst`:
**6 workers × 200 jobs, every worker attempting every job (maximal contention):**

> `exactly-once=200  double-claimed=0  conflict-retries=2  attempts-per-claim ≈ 1.01`

ADR-009 ruling #1 executed: `UPDATE job SET status='running', claimed_by=… WHERE status='queued'` is the only serialization point, and it held perfectly — zero double-claims across 1,200 contended attempts. **Rider (2) delivered:** the module-2 conflict-retry counter reports attempts-per-claim ≈ **1.01** under 6-way burst — optimistic concurrency is a non-event at queue pressure; single-row conditional updates barely conflict. The number that validates the design is how boring it is.

## Exit criterion 7 — retention cold-start rescan

`worker_queue::criterion7_coldstart_rescan_drains_queue`: a worker with no cursor and no LIVE history drains 10 queued jobs purely by `status='queued'` rescan — queue empty afterward, all 10 effects applied. ADR-009 ruling #2 executed: jobs are records, recovery is a query. Retention bounds *replay efficiency*, never correctness.

## Exit criterion 5 (hardest clause) — deny-after-revocation

`worker_queue::criterion5_revoked_authority_is_nonretryable_deny`: a job enqueued by `ghost`, whose principal is revoked before the worker runs it. The `AuthorityResolver` re-derives authority at run (identity captured, never a permission snapshot — ADR-006 edge 4), finds nothing, and the outcome is a **typed `Denied`, terminal `status='denied'`, never requeued.** Retry/deny taxonomy in the worker: permission-denied and hook-rejection are non-retryable (deterministic failures); transport/conflict-exhaustion are retryable (job returns to `queued`).

## Rider (3) — the re-entrant write, first end-to-end exercise

`worker_queue::job_effect_fires_hooks`: a `create_doc` job handler writes **through the broker** under the re-derived record-user session — and both hook classes fire inside the job effect (a 20,000 Draft comes out `Needs Approval`, the plugin's flag applied by in-process wasmtime, inside a queue job, under a record token). The job is a fresh `HookChain` causal root; nested writes from hooks remain cycle-trapped. Jobs re-derive real record sessions (seeded `app_user` + record ACCESS in the test) — not root shortcuts.

## Housekeeping

- `surql_monopoly` caught `worker.rs` on its first run — third catch, third recorded ruling (claim/finish statements, all values escaped via `surql.rs`). The allowlist now documents the justification for all seven entries inline.
- Disk hit 0 again mid-module; reclaimed 2.1 GB by deleting the wasm-spike *target* dirs (the `.wasm` artifacts the kernel loads are preserved separately in `wasm-spike/artifacts/`).

## Honest scope note

The executed, test-covered core is **claim + run + rescan** — the correctness-bearing parts of ADR-009's loop. The LIVE-tail residency (replay-from-cursor → LIVE tail → advance cursor as a long-running daemon) is thin composition of parts proven in WO-004, and it needs a resident process — which arrives with module 6's `frust serve`. Rescan is the superset-correct path; LIVE is the latency optimization on top. Carried forward explicitly, not silently.

## Suite state

**All 12 test binaries green, zero failures** — 95 (frust-orm) + kernel units + boot_discipline + conflict_canary + hook_dispatch + metadata_sync + permission_proof + surql_monopoly + worker_queue.

## Related
[[WO-005 Metadata Kernel v0]] · [[ADR-006 Plugin Capability Surface]] · [[ADR-009 Execution Model]] · [[2026-07-24 Live-query and event fidelity (WO-004)]] · [[2026-07-24 Module 4 close — hook dispatcher fold-in]]
