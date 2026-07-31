---
tags: [frust, build-log, kernel, work-order, milestone]
created: 2026-07-24
work-order: "[[WO-005 Metadata Kernel v0]]"
---

# Build Log — Module 6 + WO-005 CLOSE: `frust serve`

**The kernel exists.** One Rust binary — `frust serve` — replaces the WO-002 three-process composition with two: `frust` + `surreal.exe`. All seven WO-005 exit criteria executed, all 15 test binaries green.

## The seven exit criteria — evidence table (test-name cited)

| # | Criterion | Evidence | Result |
|---|---|---|---|
| 1 | The sentence, verbatim, on two processes | `acceptance_e2e::the_sentence_through_two_processes` | ✅ DocType created at runtime, synced to live schema, submitted through both hook classes over REST (Needs Approval @ 20k), reject → 422, audit trail read from changefeed — no restarts, no :8787, no sliver |
| 2 | One permission compiler, three consumers, byte-equal | `rest_surface::rest_is_the_third_consumer_byte_equal` + `rest_row_split_matches_broker` | ✅ REST vs direct-broker rows serde-identical for all 3 principals; 2/1/3 row split enforced by the DB via REST |
| 3 | REQ-6.1 gates hold in CI | `perf_gates::gate_submit_latency` (26 ms debug, gate 60), `gate_hook_overhead` (<1 ms, gate 30) | ✅ regression → build red; REQ-6.1.2 is now real |
| 4 | Boot discipline | `boot_discipline::*` (4 tests, module 2) | ✅ newer-DB refuses named, two-step ack, racing-nodes single-apply |
| 5 | Queue end-to-end, authority re-derived | `worker_queue::criterion5_revoked_authority_is_nonretryable_deny` + `job_effect_fires_hooks` | ✅ deny-after-revocation is typed, terminal, non-retryable; job effects fire hooks through the broker |
| 6 | Atomic claim, exactly one winner, measured | `worker_queue::criterion6_exactly_one_winner_per_job_under_burst` | ✅ 6 workers × 200 jobs: exactly-once=200, double=0, **attempts-per-claim ≈ 1.01** |
| 7 | Retention cold-start rescan | `worker_queue::criterion7_coldstart_rescan_drains_queue` | ✅ cursor-less worker drains queue by `status='queued'` rescan, misses nothing |

## Module 6 — what landed

- **`rest.rs`** — the REST surface: `/health`, `/read`, `/write`, `/aggregate`, `/enqueue`, metadata-generated, speaking the ADR-006 structured filter contract over the wire (typed-value form `{kind,v}` preserves decimal; filters as `{path,op,value}` trees, never raw strings). Errors map to HTTP status (403/404/422/500). This is the ONE permission compiler's third door.
- **`worker.rs` resident loop** — `ResidentWorker::tick()` drains claimable jobs between REST accepts; `AppUserResolver` re-derives authority from live `app_user` (revoked/disabled → non-retryable deny). ADR-009's replay/rescan → claim → run loop is now executed *resident*.
- **`frust serve`** — boot → hooks load → REST + worker in one resident process. `frust` (no `serve`) stays a one-shot boot check.
- **Desk fold-in (the literal one):** `frust-proto`'s submit and list now call the kernel's REST surface — `db::rest_write` / `db::rest_read`. The Desk holds **zero SurrealDB tokens**; signin + authenticated writes moved into the kernel. Its old hooks-to-:8787-then-CREATE dance is deleted; it is a pure `(metadata, record JSON)` REST client per ADR-004. Compiles clean.

## Findings (loud, classified — silent counter stays at 2)

- **Broker `DocTypeMeta` required `label`** — a programmatically-created doctype omitting it got a 500. Made `label` `#[serde(default)]` (display-only). A strictness mismatch between broker and sync engine, caught by the criterion-1 e2e; loud, not silent.
- The UB near-miss from module 5 (a `transmute` in a draft test) was designed away with a `claim_only` constructor — recorded here as the second "caught, not suppressed."

## WO-005 close

Five static fixes 5/5. The sliver died in module 3 and stays dead. `surql_monopoly` is 3-for-3 recorded rulings (broker, sync, worker). Two→one processes, on our name. **All SRS gaps remain closed; every exit criterion is an executed test, not a claim.**

The kernel is done. The board turns to what it unblocks: materialized aggregates, the 1M-row re-run against the real engine, P-8.2 resource isolation, Desk v1.

## Related
[[WO-005 Metadata Kernel v0]] · ADR-001…009 · [[2026-07-24 Module 5 close — worker loop]] · [[2026-07-24 Module 4 close — hook dispatcher fold-in]] · [[2026-07-24 Module 3 close — sync engine port + rollback position]]
