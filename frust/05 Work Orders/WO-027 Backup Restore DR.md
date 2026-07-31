---
tags: [frust, work-order, backup, dr, production, surrealdb]
status: COMPLETED 2026-07-27 — 39 groups/38 binaries/0 failed. Round-trip clean on uncorrupted data (4 line rows, AR 101.96, meta v4, app registered; 785/891 ms, 25.9 KB). **Security-P0 found + fixed:** restore-path auth bypass (import installs `[REDACTED]` as live signing key, forged token accepted — proven) → fail-closed boot guard, self-forge detection (introspection proven blind), no serve-anyway ack, keyguard_canary pins the constant, [[ADR-013 Signing-Key Integrity at Boot]] ratified. Guard proven 3 ways (refuse-restored / boot-healthy / boot-then-401-after-reissue). [[DR Runbook]] written, honest RTO ~1 min / RPO = export interval (NO PITR). Criterion 6: **no verdict moved** (DR isn't one of 34 pains — minting a row = grade inflation), 2 bounds TIGHTER: P-8.1 (no per-tenant restore — tenancy trade lands against us → multi-DB promoted preference→ops-requirement), P-7.1 (DR is procedure not packaging — no frust backup/restore, ~25s accepting boot). 3rd session monopoly-guard self-catch (DDL string → meta.rs). → [[2026-07-27 WO-027 backup restore DR]]
created: 2026-07-26
---

# WO-027: Backup / Restore / DR (the Adoption-Era Risk, Finally Closed)

> [!info] PM work order — sequenced ahead of Topcoat by asymmetric risk (a production DR gap has no workaround; a dependency bump's cost-of-delay is flat). Governing: [[SurrealDB]] risk list (*"backup/restore, PITR, monitoring younger than the MySQL ecosystem — define the backup story early"* — flagged at adoption, open 27 WOs). **Empirical-first: the probe decides whether this is a procedure to document or a gap to escalate.**

## Criterion 1 — THE DR PROBE, before designing any procedure

What does SurrealDB v3.2.0 (surrealkv backend) actually offer for backup and restore? Probe, don't assume:

1. **Full backup:** does `surreal export` capture a complete, restorable snapshot of a live store — schema, data, changefeed history, `DEFINE ACCESS`/JWT keys, meta-schema version? Take one against the 1 M fixture, measure time + size.
2. **Restore fidelity:** `surreal import` (or file-level restore of the data dir) into a fresh store — does it round-trip *exactly*? Prove with a known dataset: the accounting seed installed, invoices submitted, AR rollups populated → export → restore into a clean store → **every number reconciles, the app still installed, sessions/JWT state coherent, meta version intact.** This is the load-bearing test.
3. **Point-in-time / incremental:** is there any PITR story, or is it snapshot-only? (The changefeed is a mutation log — is it a recovery tool or only an audit tool? WO-011 found record sessions refused `SHOW CHANGES`; does a root restore path exist?) State honestly what exists.
4. **Live-backup safety:** can a backup be taken against a *running* kernel without stopping it, or does it require quiescing writes? (The WAL-doesn't-compact caveat and the concurrent-write path from WO-026 are both relevant.)

**If the probe finds backup/restore can't round-trip a live app faithfully, STOP — that is the adoption bet's ops limit surfacing, a production-blocking finding, and it's an ADR/ops-architecture conversation, not a workaround.** Report before building any procedure.

> [!danger] SECURITY-P0 finding + PM ruling (2026-07-28): restore-path auth bypass. `import` installs the redacted `[REDACTED]` placeholder AS the live signing key; the restored store accepts tokens forged with that published constant (proven — forged `app_user:mgr` token returned data; source store 401'd the same token). **Guard APPROVED** with shape: (1) fail-closed boot refusal, `FRUST:E_RESTORED_ACCESS_KEY`, extends ADR-008 fail-closed lineage (key-integrity, not just meta-version); (2) **NO serve-anyway ack** — proceeding = serving-compromised; the only forward path is re-issuing `DEFINE ACCESS` with a fresh key; (3) **canary-pinned detection** — probe whether the kernel can introspect the key (compare to `[REDACTED]`) OR use the positive self-forge test (at boot, forge a token with the published constant, check if the store's own access accepts it — tests the vuln directly); either way pin the SurrealDB redaction constant with a canary so a version bump fails CI loudly (WO-018 conflict-canary precedent); (4) runbook key re-issue is mandatory **regardless** of the guard (belt + braces). Produce a decision record (ADR-008 amendment or short ADR); PM ratifies. **v1.1 candidate noted:** a first-class `frust restore` that re-issues the key atomically — make the safe path the easy path.

## Remaining Criteria (shape depends on the probe)

5. **The documented DR procedure:** a written, *tested* runbook — backup command, restore command, the meta-version/JWT considerations (ADR-008 fail-closed boot, WO-008 JWT-rotation caveat both interact with restore), RTO/RPO honestly stated from the measured numbers.
6. **Re-score the relevant scorecard rows:** P-8.1/multi-tenancy ops and any "no DR story" edge the scorecard carries — this WO is licensed to edit the scorecard with the evidence it produces.
7. **Floor + footprint untouched** (this is mostly ops, but any kernel change for backup-coordination gets the full hygiene set on a fresh store).

## Boundaries

- No third-party backup infra invented — use SurrealDB's own tooling first; if it's insufficient, that's the finding, not a prompt to build a backup daemon.
- DR for the *two-process* deployment (kernel + surreal). Multi-node/distributed (TiKV) DR is out of scope — note it if the probe touches it.

## Escalations

Standard rules + full hygiene set. Criterion-1 stop-condition above is the big one: **immature DR tooling is a production finding that reopens the SurrealDB ops-risk question, not something to paper over with a fragile script.**

**Related:** [[Frust Hub]] · [[SurrealDB]] (ops-risk list) · [[ADR-002 SurrealDB Lock-In]] · [[ADR-008 Data Shape]] (fail-closed boot / meta version on restore) · [[v1.0 Pain-Point Scorecard]]
