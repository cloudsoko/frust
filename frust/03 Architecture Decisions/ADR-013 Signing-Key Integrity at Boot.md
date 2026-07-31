---
tags: [frust, adr, security, boot, dr, surrealdb]
status: ACCEPTED 2026-07-27 (WO-027; PM ratified) — extends ADR-008 fail-closed lineage from meta-version to signing-key integrity; detection is self-forge (introspection proven blind), no serve-anyway ack, canary-pinned constant, scope bounded to the restore path.
---

# ADR-013: Signing-Key Integrity at Boot (fail-closed, extended)

## Context

[[ADR-008 Data Shape]] established a fail-closed boot: *a database whose state
the binary cannot vouch for does not get served*. That principle was applied to
the **meta-schema version** — a DB newer than the binary refuses; a DB older
refuses without an explicit acknowledgement.

WO-027's DR probe found a second class of unvouchable state, with worse
consequences.

**The finding, proven not inferred.** `surreal export` redacts the record
access's JWT signing key to the literal string `[REDACTED]`. `surreal import`
restores that string **as the actual signing key**. A restored instance then:

- boots normally,
- authenticates users normally,
- serves the application normally,
- and **accepts JWTs forged by anyone who knows the constant** — which is
  anyone who has ever seen a SurrealDB export file.

Demonstrated: a token signed with `[REDACTED]` claiming `app_user:mgr` returned
invoice data from the restored store; the same token against the source store
(real key) returned `401`. The restore path *introduces* the bypass.

**Severity comes from when it fires.** Restore happens during an incident —
precisely when an operator is least likely to audit DDL and most likely to
expose the instance immediately. And nothing looks wrong. It is the
silent-wrong class, in the disaster-recovery path, with an auth-bypass
consequence.

## Decision

**The kernel refuses to serve a database that accepts the export placeholder
key.** Machine code `FRUST:E_RESTORED_ACCESS_KEY`; the error carries the exact
remediation DDL.

Three shape rulings:

### 1. No acknowledgement flag

ADR-008's `--accept-meta-migrations` exists because *proceeding is a legitimate
operator choice*. Here it is not: proceeding **is** serving-compromised. The
only forward path is re-issuing `DEFINE ACCESS` with a fresh secret. A flag
saying "I know the key is public, boot anyway" would be a footgun wearing a
safety label.

### 2. Detection is a self-forge test, not introspection — because introspection cannot work

Probed on v3.2.0: **`INFO FOR DB` reports `KEY '[REDACTED]'` for a HEALTHY
access too.** A real key and the placeholder are indistinguishable by
inspection, so a comparison-based guard would fire on every store or on none.

The guard therefore **tests the vulnerability itself**: it mints a token signed
with the published constant and asks the database whether it accepts it. A
guard that exercises the hole beats one that checks a proxy for it. The forged
identity is deliberately a *nonexistent* record — probing showed acceptance
turns only on signature validity, so the check needs no real user and works
against an empty database.

### 3. The constant is pinned by a canary

The guard's correctness rests on a SurrealDB constant. A version that changed
the redaction string would silently disable the guard and quietly reopen the
bypass — a regression that leaves everything green. `keyguard_canary` asserts
the discriminator as a *property of the running server* (a placeholder-keyed
store must accept the forged token; a real-keyed store must reject it), so a
version bump fails the build instead of the security posture. Same precedent as
the WO-018 conflict-string canary.

## Consequences

- A restored instance **will not start** until an operator re-issues the key.
  That is the intended cost: a compromised instance should not start.
- **Sessions never survive a restore** regardless — the key changes, so every
  outstanding token is invalid. This is unavoidable, and the runbook says so.
- **`access_ddl()` does not remediate.** It is `DEFINE ACCESS IF NOT EXISTS`
  (WO-008, so ordinary boots never rotate the key and never sign everyone out),
  which is a **no-op** against an existing placeholder access. The remedy must
  use `OVERWRITE`. A test asserts this, because a runbook that said "re-run the
  DDL" would be confidently wrong.
- Boot cost: one forged-token round trip. Unmeasurable against boot's existing
  meta and sync work.

## Scope

This defends the *restore* path. It does not attempt to protect against an
operator who deliberately sets a weak key — that is a different problem with a
different answer (entropy policy), and conflating them would make the guard
noisy without making it stronger.

## Related

[[ADR-008 Data Shape]] (the fail-closed lineage this extends) · [[ADR-002 SurrealDB Lock-In]] (an adoption-era ops risk, now closed) · [[SurrealDB]] (redaction caveat) · [[WO-027 Backup Restore DR]] · [[v1.0 Pain-Point Scorecard]]
