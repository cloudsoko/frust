---
tags: [frust, work-order, topcoat, dependency]
status: COMPLETED 2026-07-28 (declared in-message, file authored retroactively so links resolve)
created: 2026-07-28
---

# WO-029: Topcoat v0.5.0 Adoption

> [!info] Enabling WO — sequenced after the DR spine (flat cost-of-delay). Non-breaking for us because the 2026-07-25 pin-to-main already absorbed the labelled breaking changes. Governing: [[Topcoat]] pin governance (PM ack before rebase, pre-move state branch-preserved, verify).

## Outcome — adopted, verified live

- **Pre-move baseline preserved** at `pre-v050-20260728` (`b1e7039`); v0.5.0 release `371c740` now an ancestor.
- **The review held on every named point** — all four labelled breaking changes were already ours from the pin-to-main. **The merge surfaced a FIFTH the changelog never labelled:** `topcoat-router` `pub use content::*` → `pub mod content` (moved `Form`/`Js`/`Wasm` to `router::content::`). `Form` is upstream's own type, so this wasn't about our patches — it broke `frust-desk`'s glob import. Adapted the import, didn't patch over it. **Lesson banked: the merge is the probe — "non-breaking for us" only the build can settle.**
- **Ledger reconciled** (in [[Topcoat]]): 4 patches retired (verified upstream's replacements were supersets first — signal-utils #214 is a superset; the owned-string `push_str` narrowing was NOT, so restored as a new carried patch), 1 promoted committed (`Wasm<T>` — was uncommitted + one stash from lost, load-bearing at `frust-desk` main.rs:1325). Real divergence: 18 files / +685 lines, all ours-by-design.
- **New features in-tree:** SSE (ADR-011's push transport — unblocks retiring Desk polling) + mail prototype (v1.1 battery) both compile.

## Verification (governance-required)

- Rust suite **1125 passed / 22 failed** — the 22 a **pre-existing** CRLF artifact in `topcoat-view-grammar pretty` (fixtures `\r\n`, printer `\n`, renders identically; proven pre-existing by identical 12/22 split on the pre-move baseline). Not caused by the adoption; flagged for hygiene.
- Browser suite **11/11** (upstream's 4 new signal tests + our decimal tests).
- **Live smoke:** browser served the rebuilt `/runtime.js` carrying our Decimal surrogate; the 4.26 MB engine via `Wasm<T>` as `application/wasm` with valid magic bytes (`00 61 73 6d`) — the carried patch working through the exact seam v0.5.0 changed.

## Two verification-caught traps (both instances of standing checks)

1. **The built artifact lies.** `dist/index.js` resolved cleanly with `--theirs` and silently dropped our Decimal surrogate from the bundle while Rust still exposed `Decimal`. Rebuilt from merged sources (pnpm — npm broken). **Standing post-merge step now: rebuild committed build artifacts from merged sources.**
2. **The narrowing that bit back** (5th instance of "assert the outcome, not the operation") — dropped our `push_str` patch on "no call site uses the wider signature," an assertion from a `grep | head -20` that truncated before the one file that mattered. The suite caught it. Restored as a carried patch.

## Off-order finding (flagged, not fixed — → backlog)

Every **healthy** kernel boot now logs one `lvl:error / E_DB` line: the WO-027 keyguard's forged-token probe is a deliberately-*failing* call, and `db.rs`'s "failures ALWAYS emit" rule logs it at error level. Success-of-a-failure logging as an error undermines "errors are real" (WO-010 observability). One-line fix on the keyguard surface. → v1.1 hygiene.

**Related:** [[Topcoat]] · [[ADR-011 Realtime]] (SSE now in-tree) · [[2026-07-25 Topcoat pin moved to upstream main]] · [[Frust Hub]]
