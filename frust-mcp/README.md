# frust-mcp

`frust-mcp` is Frust's multi-user Model Context Protocol adapter. It is a
client of the documented kernel REST surface: every MCP request carries a real
Frust session token, and every read, write, workflow action, and live
subscription runs through the same kernel permission compiler as REST and the
Desk.

The server uses MCP Streamable HTTP at `http://127.0.0.1:8796/mcp`. One process
serves many MCP clients. Each initialized MCP session is bound to the bearer
token on its initialization request; all later POST, GET/SSE, and DELETE
requests must repeat that same token. A token from another principal is
rejected before dispatch.

## Install and run

Node 20+ and pnpm are required.

```powershell
pnpm install --frozen-lockfile
$env:FRUST_BASE = 'http://127.0.0.1:8795'
$env:FRUST_MCP_WRITE_EXPOSURE = '{"expense_claim":["create","update","submit","delete"]}'
pnpm start
```

Connect an MCP client to `http://127.0.0.1:8796/mcp` and set:

```text
Authorization: Bearer <kernel session token>
```

Configuration can be supplied by environment variables or a JSON file through
`FRUST_MCP_CONFIG` (see `config.example.json`). Environment variables override
the file.

| setting | default | purpose |
|---|---:|---|
| `FRUST_BASE` | `http://127.0.0.1:8795` | kernel REST base |
| `FRUST_MCP_HOST` | `127.0.0.1` | adapter bind host |
| `FRUST_MCP_PORT` | `8796` | adapter port |
| `FRUST_MCP_POLL_MS` | `250` | kernel event-drain interval |
| `FRUST_MCP_WRITE_EXPOSURE` | `{}` | JSON per-DocType write allowlist |
| `FRUST_MCP_CONFIG` | unset | path to JSON configuration |

Reads are always present. Writes are absent unless individually enabled:

```json
{
  "expense_claim": ["create", "update", "submit", "delete"],
  "mcp_activity": { "create": true }
}
```

`*` supplies wildcard defaults, and a DocType entry adds to those defaults.
The recognized verbs are `create`, `update`, `submit`, and `delete`. Each verb
is independently allowlisted per DocType: if a verb is disabled, its tool is
structurally absent from `tools/list`. Enabling `delete` registers a tool backed
only by the kernel's `DELETE /doc/{doctype}/{key}` door.

## Generated tools and wire fidelity

`GET /meta` generates `list_<doctype>` and `get_<doctype>` for every DocType,
including child DocTypes. Enabled writes add `create_<doctype>`,
`update_<doctype>`, `submit_<doctype>` where applicable, and
`delete_<doctype>`.

Delete accepts a full record ID or bare key. The kernel remains the authority:
compiled delete permission, the draft-only docstatus lattice, Single DocType
permanence, and the no-row case are all enforced there. Kernel refusals are
returned as MCP tool errors (`isError: true`) with the typed kernel `error`
object, HTTP status, and `operation_happened: false`; they are never reported as
successful deletes.

- `Table` fields are arrays of nested child objects generated from the child
  DocType named by `options`.
- `Link` fields are strings with `x-frust-link-doctype` schema hints and are
  encoded as the REST `{kind:"record",v:"..."}` wire value.
- `Select` options become JSON Schema enums.
- `Currency` is always a decimal string, recursively through child tables.
  JSON numbers are rejected before the REST call; outbound values use
  `{kind:"decimal",v:"..."}`, and inbound values are asserted to remain
  strings.

The adapter sends an `X-Trace-Id` beginning with `mcp-` on kernel calls. The
kernel adopts that ID in its `rest_request` and `broker_verb` JSON telemetry,
making an MCP-caused write visible without a separate authority header.

## Resources and subscriptions

A readable DocType is listed as a collection resource:

```text
frust://doctype/expense_claim
```

Individual rows use the resource template:

```text
frust://doctype/expense_claim/expense_claim%3Aabc123
```

The subscription mapping is faithful to both contracts:

1. `resources/subscribe` opens `POST /subscribe/{doctype}` with that MCP
   session's kernel token.
2. The adapter periodically drains `GET /events/{sub}`. Kernel ticks contain
   only `{action,id}` and have already been row-permission filtered.
3. A matching tick emits MCP `notifications/resources/updated` for the
   subscribed collection or record URI. No row payload crosses the push path.
4. The client handles the notification by reading the resource again. That
   refetch uses `/read`, so permissions are re-applied.
5. If the kernel reports `alive:false`, the adapter resubscribes and emits an
   updated notification. There is deliberately no invented replay: reconnect
   means refetch.

This is invalidation semantics, which is exactly what MCP resource-updated
notifications promise. The adapter does not claim ordered event delivery or
put row data into notifications.

## Proofs

The full proof builds `frust` from this clone, starts the specified
`D:\Dev\rust\frust-bench\surreal.exe` as an isolated in-memory store on port
**8890**, seeds the fixture, starts one kernel and one multi-user MCP server,
runs every proof, and cleans up its processes. The proof fixture has no server
scripts; kernel boot uses this clone's tracked host-compatibility guest
components from `wasm-spike/artifacts-old-world/`.

```powershell
pnpm proof
```

Runtime evidence is left under `scratch/runtime/`, including `kernel.log` for
the trace-attribution assertion. Set `FRUST_SKIP_BUILD=1` to reuse an already
built release binary. Individual scripts assume the fixture, kernel, and MCP
server are already running:

```powershell
pnpm proof:unit
pnpm proof:fidelity
pnpm proof:containment
pnpm proof:subscriptions
```

- `proof/fidelity.mjs` covers child Table, Link, Select, Currency, structural
  verb exposure, and kernel trace attribution.
- `proof/containment.mjs` holds clerk and manager principals open against one
  server and proves byte-equal REST reads, cross-principal get/filter/update
  negatives, typed delete refusal with row survival, and a successful manager
  draft delete followed by resource invalidation.
- `proof/subscriptions.mjs` proves a readable tick, silence for a manager-only
  DocType under a clerk token, manager-visible provenance for the same change,
  and reconnect-by-refetch.

No kernel or Desk source is used as an adapter implementation dependency, and
this work order does not edit either source tree.
