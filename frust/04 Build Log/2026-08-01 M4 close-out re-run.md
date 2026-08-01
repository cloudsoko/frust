---
tags: [frust, build-log, milestone-4, gate, assessment]
created: 2026-08-01
work-order: "[[WO-052 Orphan Columns]]"
status: **M4 CLOSES.** The single blocker WO-051's gate refused to close on — *an extension uninstall can leave a store that will not boot* — is fixed, regression-tested, and **converted on the store that actually had it**. Nothing else in WO-051's table was re-litigated: eleven items done, two deferred by ruling, one not-done and re-justified, all stand as recorded. Scorecard unmoved at **24 killed · 10 bounded · 0 open** — this was an implementation defect, not a pain-point verdict.
---

# Milestone 4 close-out — re-run (short form)

The WO-051 gate said no on one item. The instruction for this re-run was
narrow: *the blocker's row flips or it doesn't; the rest of WO-051's table
stands.* So this document is short on purpose, and re-checks exactly one row.

## The blocker's row

> **An extension uninstall can leave a store that will not boot**, with no
> operator remedy short of hand-editing the database.

**FLIPPED.** Evidence: [[2026-08-01 WO-052 orphan columns]].

| what the row needed | status |
|---|---|
| boot no longer refuses on an undeclared column | **done** — the column is *carried* back onto the desired schema from migration history, so the diff is empty rather than refused-and-skipped |
| the missing regression exists | **done** — uninstall → **restart**, watched red on `E_BOOT_DB` before it went green |
| the operator has a remedy | **done** — `POST /doctype/{name}/reclaim`, REQ-6.6.2 ack shape, through the migrator so history converges |
| the orphan is visible | **done** — boot report + `/metrics` (and the gauge is zeroed on reclaim, so it cannot report a column that no longer exists) |
| the dev store's hand fix becomes the mechanism | **done** — hand-declared field removed; the mutated binary refuses (exit 1, the blocker verbatim), the shipped binary boots and names `sales_invoice.crm_followup` with all four rows of data intact |
| no new boot flag | **done** — `BootOptions` is still `{holder, accept_meta_migrations}`, pinned by a test that reads the struct's own source |

The fix also closed a defect the row did not name and the literal reading of the
amendment would have shipped: a merely-tolerated orphan freezes its DocType's
schema forever, because the migrator abandons a whole resource on a refused
diff. Green boot, silent freeze. Carrying the column is what makes the DocType
keep evolving around it.

## Everything else — unchanged, and deliberately not re-argued

WO-051's disposition table stands as written: **eleven done**, **two deferred by
ruling** (PDF engine per [[ADR-014 Print Strategy]]; hook-vocabulary widening per
[[ADR-015 Cross-App Extension Model]]), **one not-done and re-justified**
(Desk→kernel streaming — the concurrency it would improve is measured clean at
~146 req/s with 48 SSE subscribers coexisting). Re-running that analysis would
be re-litigating a settled table, which the short form exists to avoid.

Two numbers from WO-051 worth carrying forward unchanged:

- **The DB write ceiling has no single authoritative figure.** Fresh-store
  readings span ~335–555 req/s on this machine; the *shape* is stable
  (DB-bound, saturates around c=2, p50 grows linearly). Anyone quoting one
  number should say which run. *"~150 w/s raw"* remains retired.
- Perf gates re-run fresh-store for WO-052, both auth modes: hook 0 ms/30,
  submit 4–5 ms/25, realtime tax 0.00–0.56 ms/2. Consistent with WO-051's 4 ms.

## Scorecard

**24 killed · 10 bounded · 0 open.** No verdict moves. The blocker was an
implementation defect in the mechanism that killed P-2.2, not evidence against
the kill — and P-2.2's row already carries its honest residue (validate-only
composition, no extension-to-extension ordering in v1). Moving a verdict for a
bug fix would be the grade inflation rule 3 forbids.

## The pattern worth keeping

Both milestone gates this project has run have failed on their first attempt,
and both failures were the gate working. M3's failure was three
bounded-by-assumption rows; M4's was one silent-state defect that only appears
one process-restart later. Each was found by *doing the thing* — WO-035's
measurement, WO-051's own restore — not by reading the code and concluding.

**M4 CLOSES.** M5's seed is unchanged from WO-051: app-building kit coupled to
topcoat #275, hook-vocabulary widening, REST-surface documentation, Desk→kernel
streaming, PDF engine on the attachment pull.

## Related
[[2026-07-31 WO-051 milestone 4 close-out]] (the gate that said no) ·
[[2026-08-01 WO-052 orphan columns]] · [[WO-052 Orphan Columns]] ·
[[v1.0 Pain-Point Scorecard]] · [[v2.0 Deployability Gate]] ·
[[ADR-008 Data Shape]]
