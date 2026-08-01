---
tags: [frust, work-order, mcp, api, milestone-5]
status: QUEUED (2026-08-01) — activates when the Boss pulls it. Probe (WO-059) done + [[ADR-017 MCP Surface]] accepted; this is the full build.
created: 2026-08-01
---

# WO-060: MCP Full Build

## Why

WO-059 proved the shape (adapter-over-REST, containment byte-equal + provenance-correct, TS/Node); ADR-017 ratified it. This builds the production MCP surface. Scope is ADR-017's "What WO-060 should scope," reproduced as the contract:

## Scope (the adapter build)

- **Every DocType**, child tables, Link/Select fields (options → enum / link-target hints) — not one hand-picked doctype.
- **MCP resources + subscriptions** over `/subscribe`+`/events` — the **untested** surface, and the one place adapter-over-REST might still strain (long-lived server-initiated notifications over the transport). Probe-within-the-build: **if REST genuinely can't express it faithfully, STOP and report** — that reopens adapter-vs-kernel *for the realtime slice only*, not the whole surface.
- **Multi-user auth** — MCP streamable-HTTP transport forwarding per-request `Authorization` into the bearer path; decide one-shared-server vs per-agent-process.
- **Per-doctype / per-verb write exposure** (finer than the one global `FRUST_MCP_WRITES` flag) + **trace attribution that a write came via the MCP consumer** — the "which consumer changed this" story (P-2.2-adjacent) extended to agents.
- The containment proof (`frust-mcp/proof/containment.mjs`) is the **permanent regression**: every consumer stays byte-equal + provenance-correct; the over-read gate must never fire.

## Separate small kernel WO (NOT this adapter build — ADR-017 Findings 1–5)

The additive kernel enrichments benefit **all BYO clients**, not just MCP, so they're their own kernel WO under the evolution policy (additive-only): `FRUST_DB_ENDPOINT` (endpoint hardcoded); `/meta` exposes `label`; `/meta` carries the workflow slot; `/read` id-filter accepts a plain string; and the **money-write coercion** (PM-ruled: the REST write path accepts a bare decimal string on a Currency field, so the documented convention holds on write too). Sequence when MCP or another BYO consumer needs them.

## Isolation & discipline

Same as WO-059: standalone `frust-mcp/`, own scratch store + port, **zero kernel/desk edits** (the kernel findings go through their own WO). If launched as a parallel agent, **pin `model: opus`** (standing rule).
