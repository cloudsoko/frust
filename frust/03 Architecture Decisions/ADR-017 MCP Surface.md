---
tags: [frust, adr, mcp, api, ai-agents, milestone-5]
status: ACCEPTED (2026-08-01) — ratified on WO-059's evidence; **PM independently re-verified the proof's PROVENANCE assertions** (clerk1 `get(clerk2-id)` = not-found, filter-at-clerk2 = 0-rows, cross-owner-update = typed-error with the row provably unchanged — the WO-039 standard, not "data returned") **+ the vault writes landed.** Adapter-over-REST confirmed. **PM ruling on Finding 2:** the REST write path SHOULD accept a bare decimal string on a Currency field (make the documented "money is a string" true on write too) — named for a small additive kernel WO. Build = [[WO-060 MCP Full Build]] (queued, Boss pulls).
created: 2026-08-01
---

# ADR-017 (proposed): MCP Surface — DocTypes as MCP Tools, Adapter-over-REST, the Fourth Consumer

## The decision this proposes

**Frust is MCP-native by generating an MCP tool surface from DocType metadata,
served by an adapter that is a CLIENT of the documented REST surface — never a
kernel endpoint.** An AI agent operates the ERP through the **same permission
compiler** as REST, the Desk, and plugins: a *fourth consumer*, byte-equal,
contained by construction. The agent cannot over-read because the boundary is
**structural (in the kernel), not in the adapter's discipline or the agent's
prompt.** This is the alpha (one compiler, N consumers) extended to agents, and
it dogfoods ADR-016's "the REST surface is a product" — an MCP client is the
canonical BYO machine consumer.

Evidence: [[2026-08-01 WO-059 mcp-native probe]] — 21/21 proof checks, driven
through the official MCP client SDK. The load-bearing one: a **clerk** session's
`tools/call` returned **exactly the clerk's rows, byte-equal to REST and
provenance-correct**; every over-read attempt (get-by-id of another owner's row,
a filter aimed at it) was refused by the kernel. The escalation condition (an
agent over-reads) **never fired**.

## The architecture fork — resolved

**Adapter-over-REST vs kernel-endpoint → adapter-over-REST CONFIRMED.** The REST
surface carried MCP faithfully for the shape MCP needs *today*: tools are
request/response, and the DocType verbs map 1:1 — `list/get → /read`,
`create/update → /write`, `transition → /transition`, schema ← `/meta`. No
protocol or streaming semantics were encountered that REST could not express, so
putting MCP in the kernel would buy nothing and cost the headless contract
(ADR-004) and kernel leanness. The kernel stays the one place the compiler runs;
the adapter inherits containment for free.

**Honest boundary of that finding:** the probe exercised **tools only**. MCP
*resources*, *prompts*, and especially **subscriptions/notifications** were not
built. The kernel's `/subscribe`+`/events` realtime path maps naturally onto MCP
resource subscriptions, and *that* is the place a future "REST can't express it
faithfully" could still surface (long-lived server-initiated notifications over
a stdio/HTTP transport). It did not surface here, but the claim is scoped to the
CRUD+transition+meta surface, not to all of MCP. → WO-060.

## Impl language — TypeScript / Node, official `@modelcontextprotocol/sdk`

Chosen and built. Reason, in order: (1) **maximal isolation** — a different
language and process, no shared cargo target, so it cannot conflict with kernel
or Desk work (WO-037 precedent); (2) **laziest** — plain `.mjs`, no build step,
Node's global `fetch`, one dependency (the SDK); (3) it **is** a real machine
client of the documented surface, which is the point. A Rust adapter buys
nothing here and re-couples the build.

## Auth-forwarding shape (the named question)

**One server instance is bound to ONE Frust principal; the credential lives in
the launch config; every `tools/call` forwards that one bearer token verbatim.**

- The agent's MCP client launches the server with `FRUST_USER`+`FRUST_PASS` (the
  server calls `/login` once and caches the `<TenantId>.<random>` session token)
  or a pre-minted `FRUST_TOKEN` — exactly like every MCP server's API key.
- Containment is that **the token IS a real kernel session**, subject to the
  compiler. The adapter cannot forge another principal's token, cannot widen a
  filter, and never re-implements a permission. `/meta` is itself
  permission-filtered, so even the *tool surface* follows the session.
- This is correct and sufficient for the stdio, single-principal deployment (one
  agent = one server = one login). **The multi-user shape** — one server, many
  agents, per-request identity — is MCP's **streamable-HTTP transport forwarding
  the caller's `Authorization` header** into `/login`/the bearer path. Same
  principle (the kernel authenticates and enforces), heavier plumbing. → WO-060.

## Decimal / money ruling

**The adapter presents money to the agent as a decimal STRING and forwards it to
the kernel in the typed `{"kind":"decimal","v":"…"}` form.** This is not gilding:
WO-059 found that on **write**, a Currency field **refuses a bare decimal string**
(`Expected decimal but found '42.00'`) even though the docs' "money is a decimal
string" convention holds on read. The typed wrap makes the agent's clean string
work *and* turns malformed money into a clean `400 invalid-value` instead of a
raw db coercion error. Ruling: **the adapter may be smarter than passthrough
exactly where it improves safety (money typing), and must NEVER be smarter where
it would relax the boundary (it forwards the token; it never decides
permissions).** ADR-007's compare-never-compute is untouched — the adapter does
no arithmetic; it re-types a value the agent already supplied.

*(This also flags a kernel-side question for the PM: should the REST write path
coerce a bare decimal string on a Currency field, so BYO clients following the
documented convention are not surprised? A gap named, not fixed — the probe does
not edit the kernel.)*

## Write-gating ruling

**Write tools are opt-in and OFF by default** (`FRUST_MCP_WRITES=on`). "An AI
agent can submit invoices" is a deliberate exposure, so with writes off the
`create`/`update`/`transition` tools are **not registered** — an agent cannot
call a tool that does not exist (structural, not a runtime guard). When enabled,
every write flows through the broker (hooks fire, the docstatus lattice holds),
and **a refused write is a typed tool error (`isError`) carrying the kernel's
`{kind, detail}` — never a silent "created"** (the WO-057 shape). Proven:
cross-owner update → `403 permission-denied` and the target row provably
unchanged; wrong-state transition → `422 workflow-denied`; malformed money →
`400 invalid-value`.

## Findings folded in (from the probe)

1. **DB endpoint hardcoded** (`ConnConfig::default` → `127.0.0.1:8899`, no env).
   Recommend an additive `FRUST_DB_ENDPOINT` so BYO/MCP deployments can select a
   store. (Shaped WO-059's isolation: unique database, not a separate process.)
2. **Money write wrinkle** — see the ruling above.
3. **`/meta` omits `label`** → descriptions synthesised; recommend additively
   exposing `label`.
4. **`/meta` omits the workflow slot** → a transition tool can't enumerate
   actions from meta; recommend additively carrying the workflow definition.
5. **`/read` id-filter needs a typed `{kind:"record"}` value** (a string matches
   nothing). Minor; documented.

None of 1–5 breaks the thesis; all are additive, evolution-policy-safe changes.

## What WO-060 (the full build) should scope

- **Every DocType**, child tables, and Link/Select fields (options → enum /
  link-target hints), not one hand-picked doctype.
- **MCP resources + subscriptions** over `/subscribe`+`/events` — the untested
  surface, and the one place adapter-over-REST might still strain.
- **Multi-user auth** — streamable-HTTP transport forwarding per-request
  `Authorization`; decide whether one shared server or per-agent processes is the
  deployment.
- **Per-doctype / per-verb write exposure policy** (finer than one global flag)
  and an audit note that a write came via the MCP consumer (trace attribution).
- Decide the **kernel-side money-coercion** question (Finding 2) and the four
  additive `/meta`/endpoint improvements (Findings 1,3,4,5) — as kernel work,
  not adapter work.

## Sequencing

Behind ADR-016's follow-through (REST-surface docs already exist — WO-054),
since MCP is the canonical proof that the documented surface is a real product.
The probe is done and green; the full build is a clean M5 candidate whenever the
Boss pulls it.
