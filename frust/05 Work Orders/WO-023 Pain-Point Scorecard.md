---
tags: [frust, work-order, scorecard, v1.0, milestone]
status: COMPLETED 2026-07-26 — **19 killed · 14 bounded · 1 open** of 34, every verdict vault-linked. P-1.4 (memory) is the lone open — never measured, so not claimed (the anti-inflation proof). Security P-5.x = clean sweep, hostile-first. Bounded held honest: P-3.4 (script-decimal footgun the seed tripped on), P-8.2 (measured 1.4×), P-1.1 (single-threaded serve, unbenchmarked). Verdict: a Frappe replacement for the pains that drove the rewrite, provably — and honestly not yet turnkey (no high-concurrency serve, no memory number, hand-rolled app decimal, no batteries). v1.1 backlog seeded from the remainder. → [[v1.0 Pain-Point Scorecard]]
created: 2026-07-26
---

# WO-023: The v1.0 Pain-Point Scorecard

> [!info] PM work order — **assessment, not a build.** The founding document [[Frappe Pain Points]] becomes Frust's acceptance test. This is the moment the vault was built toward since its first note: walk every `P-x.x`, score it against the evidence 22 build WOs produced.

## The task

Produce a scorecard note (`01 Vision/v1.0 Scorecard.md`) that scores **every one of the 34 pain points** in [[Frappe Pain Points]] with:

- **Verdict:** `KILLED` / `BOUNDED` / `OPEN` — three verdicts, and *bounded is honest, not a euphemism*. P-8.2 is bounded at 1.4×, not killed. Float-money is killed at boundaries but the WO-022 finding shows app-script computation still hand-rolls it (P-3.4 is bounded, not killed — the footgun moved, it didn't vanish).
- **Evidence:** a link to the build log / ADR / WO that earns the verdict. A verdict without a vault link is an opinion, and this document does not carry opinions.
- **The honest column:** where `BOUNDED`/`OPEN`, one line on *what remains* — this becomes the v1.1 backlog, seeded directly from the scorecard.

## Rules of scoring

1. **No verdict without evidence in the vault.** If a pain point was never tested, it's `OPEN`, not `KILLED`-by-assumption — the tested-seam≠wired pattern (CLAUDE.md) means "we built it" isn't "it works in the product." Prefer the verdict backed by a *live/browser* proof over a broker-test one.
2. **Cross-check the SRS:** every `P-x.x` should trace to the REQ(s) that address it and the WO that proved it — gaps in that chain are findings.
3. **Count the score honestly:** X killed / Y bounded / Z open. A large "bounded" count is not a failure — it's a rewrite two weeks old being truthful about its edges. An inflated "killed" count would betray the whole method.
4. **Name the known-open going in** (do not let them hide): P-8.2 (bounded 1.4×), P-3.4 (bounded — script-decimal footgun, WO-022 F1), workflow-Desk-UX (WO-018 c5, WO-022 F5), and anything the walk surfaces.

## Exit Criteria

1. All 34 pain points scored, each with a verdict + evidence link + (where not killed) a one-line remainder.
2. The tally stated plainly at the top.
3. A short **v1.1 backlog** section = every `BOUNDED`/`OPEN` remainder collected, so the next milestone has its inputs.
4. A one-paragraph honest verdict on the founding question: *is this a Frappe replacement, and where isn't it yet?*

## Boundaries

- This WO writes no product code. If scoring reveals a bug, that's a finding → its own WO, not a fix smuggled into the assessment.
- Resist grade inflation. The value of this document is that it can be trusted; a scorecard that scores everything killed is worth nothing.

**Related:** [[Frust Hub]] · [[Frappe Pain Points]] · [[SRS]] · all ADRs · all build logs (this is their reckoning)
