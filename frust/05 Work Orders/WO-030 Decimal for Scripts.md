---
tags: [frust, work-order, money, scripts, v1.1]
status: COMPLETE (2026-07-28) — P-3.4 bounded→killed; Decimal binding = decimal.rs compiled into the Boa sandbox verbatim; three hosts one answer proven (incl. 0.335); seed recon rewritten, hand-rolled math deleted; WO-017 guard + containment intact; both shared-artifact surfaces rebuilt. See [[2026-07-28 WO-030 decimal for scripts]].
created: 2026-07-28
---

# WO-030: Decimal for Scripts (Close the P-3.4 Footgun)

> [!info] PM work order — first v1.1 item, chosen for best gate-advance-per-effort: it moves **P-3.4 bounded→killed**, the machinery already exists (`decimal.rs` built WO-016/021, script host built WO-017), and the dogfood *itself* proved the need — the accounting seed's reconciliation script hand-rolled integer-minor decimal math and **my (PM's) own first version had a scale bug** (0.335→0.33). Governing: [[SRS]] REQ-6.2, [[ADR-007 Tier-2 Script Architecture]] (the script host), [[2026-07-26 WO-022 accounting seed]] (F1, the footgun).

## The problem (from WO-022 F1)

App scripts compute money in JS floats or hand-rolled integer-minor arithmetic, because `decimal.rs` is kernel-only and unreachable from the Boa sandbox. So every app author re-implements decimal multiply + half-even rounding, and the failure mode is silent wrong money (the scorecard's P-3.4 bound). The arithmetic exists and is CI-proven byte-equal to the DB (WO-021 `money_reconciliation`) — it just isn't exposed where apps compute.

## Exit Criteria

1. **A decimal API in the script host:** the Boa sandbox gets a `Decimal` binding (construct from string, `mul`/`div_round`/`add`/`round(scale, mode)`) backed by `decimal.rs` — the *same* implementation the kernel and DB reconcile against, not a JS reimplementation. A script computing `qty × rate` gets the exact kernel answer.
2. **Rounding stays explicit (REQ-6.2.2 holds in the script surface):** no implicit rounding; mode is explicit; a script that would produce a fractional-cent result must round at a defined point, same contract as `decimal.rs`. The WO-017 decimal-catch (script-mangled money refused typed) stays intact — this *adds* a safe path, it doesn't remove the guard.
3. **Byte-equal to the other two hosts:** a script computing a line total, the kernel computing it, and the DB EVENT computing it all produce the *same* decimal string — extend the WO-021 reconciliation property to the script host (three hosts, one answer).
4. **The seed's reconciliation script rewritten to use it** — replace the hand-rolled integer-minor math with the `Decimal` binding, prove the scale bug that bit the PM cannot recur (a test with `0.335`-family values that the hand-rolled version got wrong).
5. **Re-score P-3.4:** bounded→killed with evidence, or a stated reason it stays bounded (e.g. if the binding can't cover a needed op). This WO is licensed to edit the scorecard.
6. **Sandbox containment intact:** the decimal binding must not become an escape hatch — it's pure computation, no I/O, no capability beyond arithmetic. The WO-017 hostile-first posture (loop/hog/escape all contained) must still hold with the binding present.

## Boundaries

- Arithmetic only — this exposes `decimal.rs`, it does not build a money-formatting/currency-display library (that's presentation, a separate v1.1 item if wanted).
- Reuse `decimal.rs` verbatim across the FFI/host boundary; do NOT reimplement decimal in JS (that reintroduces exactly the divergence this closes).

## Escalations

Standard rules + full hygiene set (fresh store, dedicated scratch dir, quiet machine for any perf gate). If exposing `decimal.rs` into Boa needs a host-binding shape that doesn't cleanly round-trip decimal strings, report it — a lossy binding is worse than none.

**Related:** [[Frust Hub]] · [[SRS]] (REQ-6.2) · [[ADR-007 Tier-2 Script Architecture]] · [[2026-07-26 WO-021 money arithmetic]] · [[2026-07-26 WO-022 accounting seed]] (F1) · [[v1.0 Pain-Point Scorecard]] (P-3.4)
