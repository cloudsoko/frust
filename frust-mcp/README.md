# frust-mcp — DocTypes as MCP tools (WO-059 probe)

A **Model Context Protocol server that is a CLIENT of the Frust REST surface.**
It reads `GET /meta`, generates MCP tools from the DocType metadata, and
translates `tools/call` into `/read` `/write` `/transition` — **forwarding the
caller's bearer token** and leaving *all* permission enforcement in the kernel.

That is the whole thesis: an AI agent operates the ERP through the **same
permission compiler** as REST, the Desk, and plugins — a **fourth consumer**,
byte-equal, **contained by construction**. A clerk-session agent *cannot*
over-read, because the boundary is structural (in the kernel), not in this
adapter's discipline.

This is a **probe** (WO-059), not a shipped server. It gates
[ADR-017](../frust/03%20Architecture%20Decisions/ADR-017%20MCP%20Surface.md).

## Layout

```
src/rest.mjs           thin HTTP+JSON client of docs/rest-api.md (zero deps)
src/server.mjs         the MCP server: /meta -> tools, tools/call -> REST
proof/containment.mjs  the containment proof, driven by the official MCP client
scratch/               a runnable kernel: seed + a COPY of frust.exe + logs
```

Nothing here touches kernel or Desk source. It is an additive, standalone
consumer — a different language, a different process, a different build target.

## Design in one screen

- **Auth-forwarding shape.** One server instance is bound to **one Frust
  principal.** The credential lives in the launch config — like every MCP
  server's API key — as `FRUST_USER`+`FRUST_PASS` (log in once, cache the
  session token) or a pre-minted `FRUST_TOKEN`. Every `tools/call` forwards that
  one token. The server can't forge another; the token **is** a kernel session.
- **Writes are opt-in, OFF by default** (`FRUST_MCP_WRITES=on`). With writes off,
  `create`/`update`/`transition` tools are **not even registered** — an agent
  cannot call a tool that does not exist.
- **A refused write is a typed tool error** (`isError: true`) carrying the
  kernel's `{kind, detail}` — never a silent "created".
- **Money** is presented to the agent as a decimal **string** and forwarded to
  the kernel in the **typed** `{kind:"decimal","v":"…"}` form (a bare string is
  refused on write — see the finding in the build log).

## Run it

The kernel binary hardcodes its SurrealDB endpoint to `127.0.0.1:8899`, so you
need a SurrealDB 3.2.3 there (root:root). Isolation is by a uniquely-named
database `frustmcp` + the kernel's own port `8795`.

```bash
# 1. seed the three principals (identities are write-closed — no REST door)
surreal.exe sql --endpoint ws://127.0.0.1:8899 -u root -p root \
  --auth-level root --multi < scratch/seed.surql

# 2. start an ISOLATED kernel on port 8795, database frustmcp (a COPY of frust.exe)
FRUST_ADDR=127.0.0.1:8795 FRUST_TENANT=frustmcp FRUST_TENANCY=database-per-tenant \
FRUST_ARTIFACTS=D:/Dev/rust/wasm-spike/artifacts \
FRUST_MAIL=file FRUST_MAIL_DIR=./scratch/mail-outbox \
./scratch/frust.exe serve          # wait for GET /ready on :8795

# 3. build the fixture through the REST door (app install + rows as each clerk)
bash scratch/seed-fixture.sh

# 4. install deps and run the containment proof (npm is broken on this box — pnpm)
pnpm install
node proof/containment.mjs         # 21/21 PASS

# 5. or drive it by hand with the MCP Inspector
FRUST_USER=clerk1 FRUST_PASS=pw-clerk1 FRUST_MCP_WRITES=off \
  pnpm dlx @modelcontextprotocol/inspector node src/server.mjs
```

Point any MCP client (Claude Desktop, the Inspector, an agent) at
`node src/server.mjs` with `FRUST_USER`/`FRUST_PASS` in the env, and it gets a
tool per DocType verb — read-only until you opt into writes.

## What the probe proved (and did not)

Proved: schema fidelity for the CRUD+transition shape; **byte-equal +
provenance-correct** reads for a clerk (no over-read, escalation-gated);
opt-in/typed-refusal writes through the broker with the docstatus lattice
holding. Did **not**: MCP resources/prompts/subscriptions, realtime
(`/subscribe`+`/events`), child tables, Link/Select fields, multi-tenant
per-request auth. Those are WO-060 scope. See the build log and ADR-017.
