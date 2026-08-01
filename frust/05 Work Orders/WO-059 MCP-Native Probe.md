---
tags: [frust, work-order, mcp, api, probe, milestone-5, parallel-track]
status: DONE (2026-08-01) — background agent delivered; **PM re-verified the containment proof's provenance assertions + the vault writes independently — PASS holds.** Adapter-over-REST confirmed, TS/Node MCP-SDK, one-server-one-principal auth; containment 21/21 (byte-equal + provenance-correct; the over-read escalation gate NEVER fired); 5 additive findings. [[ADR-017 MCP Surface]] ACCEPTED. Build = [[WO-060 MCP Full Build]] (queued). Isolation held: `frust-mcp/` standalone sibling, zero kernel/desk edits by the probe.
created: 2026-08-01
---

# WO-059: MCP-Native — DocTypes as MCP Tools (Probe)

## Why — and the alpha fit, which is exact

Frust generates REST CRUD from DocType metadata: **one permission compiler, three consumers** (REST / Desk / plugin `db-read`), byte-equal. A DocType is *already* most of an MCP tool definition — name + fields → the tool's input schema, the CRUD/transition verbs → the tool actions. So "generate MCP from DocTypes like it generates CRUD" is mechanically the same shape, and it makes Frust **MCP-native**: an AI agent operates the ERP through tools that flow through the **same broker under the same session**, so the agent is subject to the **identical permission compiler** — a fourth consumer of the one compiler.

That's not a convenience feature; it's the **alpha extended to AI agents.** Most MCP integrations are hand-written wrappers that re-implement or bypass auth. Frust's would be *generated and contained by construction* — **a clerk-session agent cannot over-read, because it's the same compiler, not the wrapper's discipline.** The strategic line: *the ERP an AI agent can safely operate, because the permission boundary is structural (in the kernel) not in the agent's prompt or the adapter's code.* Timely, differentiated, and it falls out of what's already built.

It's also the **canonical BYO client** — it dogfoods ADR-016's "the REST surface is a product" claim by being a real machine consumer of it.

## The architecture fork (the probe's first question)

**Adapter-over-REST vs kernel-endpoint.** Lean: an **MCP server that is a CLIENT of the documented REST surface** — pulls `/meta` to generate tool schemas, translates `tools/call` → `/read` `/write` `/transition`, **forwards the caller's bearer token**, and leaves permission enforcement in the kernel. ADR-consistent (ADR-004 headless, ADR-016 BYO), keeps the kernel lean, inherits containment for free. The MCP protocol layer is most-lazily the official MCP SDK (TS or Python) — a non-Rust client of the REST API, maximal isolation, minimal code. The probe **confirms or overturns this** before any full build; if MCP genuinely needs to live in the kernel, that's the finding.

## Isolation (parallel-agent, WO-037 precedent — non-negotiable)

- A **new artifact** (`frust-mcp/`, its own project) that touches **ZERO kernel source and ZERO Desk source** — a new consumer, additive-only, so it *cannot* conflict with WO-057 (kernel) or WO-058 (Desk). Source isolation is by construction.
- Runs against its **own kernel instance on a scratch store + own port** — never the dev store the WO-057/058 builder uses. If it's a separate cargo project (not a workspace member) or a TS/Python server, it shares no build target either.

## Probe criteria (predictions stated first — WO-019 template)

1. **Metadata→tool-schema fidelity:** DocType `/meta` generates faithful MCP **read** tools (list, get) — types map, `required` carried, field labels → param descriptions, and **the decimal-as-string money convention survives into the schema** (the agent must be told money is a string, never a float — the money-safety concern in a new place).
2. **The alpha — fourth consumer, the load-bearing proof:** an MCP `tools/call` under a **clerk** session returns **exactly the clerk's rows — byte-equal to REST `/read` for the same principal.** Assert **provenance** (whose rows came back), never just "data returned" (the WO-039 lesson). This is the whole point: the agent can't over-read because it's the same compiler.
3. **Write tools, gated — not automatic:** create/update/transition as tools is a **deliberate** exposure ("an AI agent can submit invoices" is a decision, not a default). Prove (a) a write tool flows through the broker — permission-enforced, hooks fire, the lattice holds; (b) exposure is **opt-in** (config or per-doctype), off by default; (c) a **refused write is a typed tool error, never a silent "created"** (the WO-057 shape — an agent must not be told it created what it didn't).
4. **Isolation proven:** `frust-mcp/` new; grep/git shows zero kernel/desk source touched; own scratch store + port.
5. **Architecture finding → ADR:** adapter-over-REST confirmed or overturned, the **auth-forwarding shape named** (how an MCP client authenticates → how the token reaches the kernel), impl-language chosen with reason. Deliverable = position paper → **ADR-017**, then **WO-060 = the full build** scoped from what the probe touched.

## Boundaries

- **Probe, not full build:** the accounting seed's DocTypes as tools, an MCP client (Claude or the SDK's inspector) driving them, the architecture answered. Not every DocType, not a marketplace, not a shipped server.
- **No kernel/desk source changes.** If the probe finds MCP genuinely needs a kernel capability `/meta` doesn't expose, that's a **finding for the ADR**, not a kernel edit inside a probe (WO-022 rule).

## Escalation

- If the containment property does **not** hold byte-equal for the MCP consumer (an agent over-reads), **STOP** — that breaks the alpha and reopens whether MCP can be a safe consumer at all. It's the finding that matters most.
- If the adapter-over-REST shape can't carry MCP faithfully (protocol/streaming semantics the REST surface can't express), STOP and report — that's the architecture conversation the probe exists to have.
