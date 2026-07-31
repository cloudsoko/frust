---
tags: [frust, work-order, apps, kernel, measurement, v2.0-gate]
status: COMPLETE (2026-07-28) - A1: a bundle hooking a DocType it does not declare is REFUSED at install, loudly (no silent last-writer-wins); two apps hooking their OWN doctypes coexist with no cross-talk -> P-2.2 becomes bounded-by-architecture (one DocType = one owning app = one script). A2: an installed app (DocType+hook+rollup+data) survives the ADR-008 two-step major upgrade FUNCTIONAL - its hook still fires -> P-7.3 becomes bounded-by-measurement. Gate assumption bucket now EMPTY. See [[2026-07-28 WO-036 multi-app composition and upgrade survival]].
created: 2026-07-28
---

# WO-036: Multi-App Hook Composition + Upgrade Survival (Gate Assumptions A1 & A2)

> [!info] PM work order — closes gate-blockers **A1** and **A2** ([[v2.0 Deployability Gate]]). Bundled because both are kernel-side "install app(s), do a thing, assert" tests sharing the live-kernel harness (`session_cache_revocation.rs`-style), both small, both closing a v2.0-gate assumption. Sequenced after WO-035 (lower architectural risk — likely confirmations, but the gate refuses "likely").

## Item A1 — Multi-app hook composition (P-2.2)

The scorecard said hook composition across apps was "lightly exercised." The gate found it is **not exercised at all**: the only two-app test (`meta_cache_invalidation`) installs hookless bundles (0 `server_scripts`) for cache-key testing. P-2.2's `bounded` rests on an untested assumption.

**Prove:** install **2+ apps, each with a server-script hook on the same DocType/hook-point**, and drive one write. Assert both hooks fire, in a defined order, composing correctly — and that the ADR-006 cycle-trap and hook-chain semantics hold across apps (one app's hook writing a field another app's hook reads/echoes, per the WO-028 full-document contract). If composition order is undefined or a cross-app interaction misbehaves, that's a finding.

## Item A2 — Major-upgrade-with-app survival (P-7.3)

The meta-migration gate is tested in isolation: `accept_meta_migrations_two_step` runs `NoUserSync` with **no app installed**. "An installed app survives a major meta-schema upgrade" is untested — the exact Frappe pain (P-7.3, upgrades break custom apps) the verdict claims to bound.

**Prove:** install an app (DocTypes + hooks + a workflow — the accounting seed shape), then drive a **major meta-schema upgrade** (the `--accept-meta-migrations` two-step, ADR-008), and assert the app's DocTypes, data, hooks, and workflow all survive functional — a write still fires its hook, a rollup still reconciles, post-upgrade. If the upgrade orphans or breaks app state, that's a finding (and a real one — it's the founding pain).

## Exit Criteria

1. A1 proven: 2+ apps with hooks compose on one write, order defined, cross-app cycle/full-doc semantics hold.
2. A2 proven: an installed app (seed shape) survives a major upgrade functional — hook fires + rollup reconciles post-upgrade.
3. Both are **committed re-runnable tests**, not one-time observations.
4. Full suite green; findings (if any) named, not fixed here (measurement WO).
5. Gate assumptions A1, A2 struck from [[v2.0 Deployability Gate]].

## Escalations

Standard rules. A1 or A2 revealing a real defect (undefined composition order, upgrade breaking app state) is a **finding → its own fix WO** — this WO measures the assumption, the fix is separate. A2 breaking is the more serious (it's the founding pain P-7.3); escalate before assuming a workaround.

**Related:** [[Frust Hub]] · [[v2.0 Deployability Gate]] (A1, A2) · [[ADR-006 Plugin Capability Surface]] (hook composition/cycle) · [[ADR-008 Data Shape]] (meta upgrade) · [[2026-07-27 WO-028 full-document hooks]] (cross-app full-doc contract)
