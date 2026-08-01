#!/usr/bin/env node
// frust-mcp — a Model Context Protocol server that is a CLIENT of the Frust
// REST surface (WO-059 probe). It generates MCP tools from GET /meta and
// translates tools/call into /read /write /transition, forwarding the caller's
// bearer token. ALL permission enforcement stays in the kernel: this process is
// the fourth consumer of the one permission compiler, byte-equal to REST/Desk/
// plugin — an AI agent cannot over-read, because containment is structural (in
// the kernel), not in this adapter's discipline.
//
// Auth-forwarding shape (the ADR-017 question): one server instance is bound to
// ONE Frust principal. The credential lives in the launch config — exactly like
// every other MCP server's API key — as FRUST_USER/FRUST_PASS (the server logs
// in once and caches the session token) or a pre-minted FRUST_TOKEN. Every
// tools/call forwards that one token. The server cannot forge another; the
// token IS a real kernel session, subject to the compiler.
//
// Writes are OPT-IN and OFF by default (FRUST_MCP_WRITES=on): "an AI agent can
// submit invoices" is a deliberate decision, so create/update/transition tools
// are not even registered unless writes are enabled — an agent cannot call a
// tool that does not exist.
//
// Diagnostics go to STDERR only; STDOUT is the MCP JSON-RPC channel.

import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { ListToolsRequestSchema, CallToolRequestSchema } from "@modelcontextprotocol/sdk/types.js";
import { FrustRest, bareKey } from "./rest.mjs";

const log = (...a) => console.error("[frust-mcp]", ...a);

// ── config from the launch environment ──────────────────────────────────────
const BASE = process.env.FRUST_BASE || "http://127.0.0.1:8795";
const USER = process.env.FRUST_USER;
const PASS = process.env.FRUST_PASS;
const TENANT = process.env.FRUST_TENANT || undefined;
const TOKEN = process.env.FRUST_TOKEN;
const WRITES = /^(1|on|true|yes)$/i.test(process.env.FRUST_MCP_WRITES || "");

// ── metadata -> JSON Schema (the fidelity surface, criterion 1) ──────────────
// NOTE: GET /meta exposes fieldname/fieldtype/options/required but NOT the
// human label — so descriptions are synthesised from fieldname+fieldtype.
function propForField(f) {
  const ft = f.fieldtype;
  const p = {};
  let desc = `${f.fieldname} (${ft})`;
  switch (ft) {
    case "Currency":
      p.type = "string";
      desc = `${f.fieldname} — MONEY. Send a decimal STRING, never a JSON float: "42.00", not 42.0. ` +
        `(The adapter forwards it to the kernel in the typed decimal form; a bare float would be refused. ` +
        `On read, "25.00" comes back "25" — trailing zeros are stripped; pad for display, never round.)`;
      break;
    case "Int": p.type = "integer"; break;
    case "Float": p.type = "number"; break;
    case "Check": p.type = "boolean"; break;
    case "Select":
      p.type = "string";
      if (Array.isArray(f.options) && f.options.length) p.enum = f.options;
      break;
    case "Link":
      p.type = "string";
      if (Array.isArray(f.options) && f.options.length) desc += ` — link to ${f.options.join("/")}`;
      break;
    default: // Data, Text, Long Text, Date, Datetime, ...
      p.type = "string";
  }
  p.description = desc;
  return p;
}

function moneyFieldSet(dt) {
  return new Set(dt.fields.filter((f) => f.fieldtype === "Currency").map((f) => f.fieldname));
}

function toolsForDoctype(dt, { writes }) {
  const name = dt.name;
  const tools = [];

  // ── read tools (always on) ──
  tools.push({
    name: `list_${name}`,
    description:
      `List ${name} records. Row permissions are enforced by the Frust kernel under the CALLER'S OWN session — ` +
      `you receive only the rows this session may read, never more. A filter cannot widen that.`,
    inputSchema: {
      type: "object",
      properties: {
        filter: {
          type: "object",
          description: "ADR-006 structured filter {path, op, value} (e.g. op 'eq'). Query text is not accepted anywhere.",
          properties: { path: { type: "string" }, op: { type: "string" }, value: {} },
        },
        fields: { type: "array", items: { type: "string" }, description: "projection; omit for all readable fields" },
        order: { type: "object", properties: { path: { type: "string" }, dir: { type: "string", enum: ["asc", "desc"] } } },
        limit: { type: "integer" },
        start: { type: "integer" },
      },
      additionalProperties: false,
    },
  });
  tools.push({
    name: `get_${name}`,
    description: `Get one ${name} by id. Returns the row only if this session may read it; otherwise an empty result (never another principal's row).`,
    inputSchema: {
      type: "object",
      properties: { id: { type: "string", description: `record id, e.g. "${name}:abc123" (bare key accepted too)` } },
      required: ["id"],
      additionalProperties: false,
    },
  });

  if (!writes) return tools;

  // ── write tools (opt-in) ──
  const props = {};
  const required = [];
  for (const f of dt.fields) {
    props[f.fieldname] = propForField(f);
    if (f.required) required.push(f.fieldname);
  }
  tools.push({
    name: `create_${name}`,
    description:
      `Create a ${name}. The write flows through the kernel broker: app hooks fire and the permission compiler decides. ` +
      `A refused write returns an ERROR (isError), never a false "created".`,
    inputSchema: { type: "object", properties: props, required, additionalProperties: false },
  });
  const upProps = { record: { type: "string", description: "id (or bare key) of the record to update" }, ...structuredClone(props) };
  tools.push({
    name: `update_${name}`,
    description:
      `Update fields of an existing ${name} (partial). Permission-enforced by the kernel; ` +
      `a refused update is an error, not a silent no-op.`,
    inputSchema: { type: "object", properties: upProps, required: ["record"], additionalProperties: false },
  });
  if (dt.submittable) {
    tools.push({
      name: `transition_${name}`,
      description:
        `Move a ${name} through its workflow. 'action' is a workflow action name (e.g. Submit, Approve). ` +
        `The actions available depend on the record's state and the caller's role — a wrong one returns a typed refusal ` +
        `(workflow-denied). The docstatus lattice (0 draft -> 1 submitted -> 2 cancelled) is enforced by the kernel.`,
      inputSchema: {
        type: "object",
        properties: { record: { type: "string" }, action: { type: "string" } },
        required: ["record", "action"],
        additionalProperties: false,
      },
    });
  }
  return tools;
}

// wrap Currency values into the kernel's typed decimal form. This absorbs a
// real BYO wrinkle: on write, a Currency field REFUSES a bare decimal string
// ("Expected decimal but found '42.00'"); the accepted forms are the typed
// {kind:decimal,v} or a bare integer. The agent still just sends "42.00".
function buildDoc(moneySet, args) {
  const doc = {};
  for (const [k, v] of Object.entries(args)) {
    if (v === undefined) continue;
    if (moneySet.has(k) && v !== null && typeof v !== "object") {
      doc[k] = { kind: "decimal", v: String(v) };
    } else {
      doc[k] = v;
    }
  }
  return doc;
}

function ok(json) {
  return { content: [{ type: "text", text: JSON.stringify(json, null, 2) }] };
}
function refused(r, note) {
  return {
    content: [
      {
        type: "text",
        text:
          `${note}\nThe kernel refused (HTTP ${r.status}). This is the permission/validation decision, ` +
          `forwarded verbatim — the operation did NOT happen.\n` +
          JSON.stringify(r.json, null, 2),
      },
    ],
    isError: true,
  };
}

async function main() {
  const rest = new FrustRest(BASE);

  if (TOKEN) {
    rest.token = TOKEN;
    log(`using pre-minted FRUST_TOKEN (prefix ${String(TOKEN).split(".")[0]})`);
  } else if (USER && PASS) {
    const who = await rest.login(USER, PASS, TENANT);
    log(`logged in as ${who.user} (role ${who.role}, tenant ${who.tenant})`);
  } else {
    log("FATAL: set FRUST_USER + FRUST_PASS (or FRUST_TOKEN). Exiting.");
    process.exit(2);
  }

  // Discover the schema and generate tools — for THIS principal. /meta is
  // already permission-filtered, so the tool surface itself follows the session.
  const m = await rest.meta();
  if (!m.ok) {
    log(`FATAL: GET /meta failed (HTTP ${m.status}): ${JSON.stringify(m.json)}`);
    process.exit(2);
  }
  const doctypes = m.json.doctypes || [];
  const byName = new Map();
  const money = new Map();
  let tools = [];
  for (const dt of doctypes) {
    byName.set(dt.name, dt);
    money.set(dt.name, moneyFieldSet(dt));
    tools = tools.concat(toolsForDoctype(dt, { writes: WRITES }));
  }
  log(`generated ${tools.length} tools from ${doctypes.length} doctype(s); writes ${WRITES ? "ENABLED" : "OFF (read-only)"}`);

  const server = new Server(
    { name: "frust-mcp", version: "0.0.1" },
    { capabilities: { tools: {} } }
  );

  server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools }));

  server.setRequestHandler(CallToolRequestSchema, async (req) => {
    const { name, arguments: args = {} } = req.params;
    const us = name.indexOf("_");
    const verb = name.slice(0, us);
    const dtName = name.slice(us + 1);
    const dt = byName.get(dtName);
    if (!dt) return { content: [{ type: "text", text: `unknown tool: ${name}` }], isError: true };
    const moneySet = money.get(dtName);

    try {
      switch (verb) {
        case "list": {
          const body = {};
          for (const k of ["filter", "fields", "order", "limit", "start"]) if (args[k] !== undefined) body[k] = args[k];
          const r = await rest.read(dtName, body);
          return r.ok ? ok(r.json) : refused(r, `list_${dtName} failed.`);
        }
        case "get": {
          // /read's id filter matches a RECORD, not a string — a bare string
          // value returns nothing (SurrealDB `id` is a record link). The typed
          // {kind:record,v} form is what matches (a BYO finding).
          const fullId = String(args.id).includes(":") ? String(args.id) : `${dtName}:${args.id}`;
          const r = await rest.read(dtName, { filter: { path: "id", op: "eq", value: { kind: "record", v: fullId } }, limit: 1 });
          if (!r.ok) return refused(r, `get_${dtName} failed.`);
          const rows = r.json.rows || [];
          if (rows.length === 0)
            return ok({ found: false, note: `no ${dtName} '${args.id}' visible to this session` });
          return ok({ found: true, row: rows[0] });
        }
        case "create": {
          if (!WRITES) return { content: [{ type: "text", text: "writes are disabled" }], isError: true };
          const r = await rest.write(dtName, { doc: buildDoc(moneySet, args) });
          return r.ok ? ok({ action: r.json.action, record: r.json.record, row: r.json.created }) : refused(r, `create_${dtName} refused.`);
        }
        case "update": {
          if (!WRITES) return { content: [{ type: "text", text: "writes are disabled" }], isError: true };
          const { record, ...rest_args } = args;
          const r = await rest.write(dtName, { record: bareKey(record), doc: buildDoc(moneySet, rest_args) });
          return r.ok ? ok({ action: r.json.action, record: r.json.record, row: r.json.created }) : refused(r, `update_${dtName} refused.`);
        }
        case "transition": {
          if (!WRITES) return { content: [{ type: "text", text: "writes are disabled" }], isError: true };
          const r = await rest.transition(dtName, bareKey(args.record), args.action);
          return r.ok ? ok(r.json) : refused(r, `transition_${dtName} '${args.action}' refused.`);
        }
        default:
          return { content: [{ type: "text", text: `unknown verb in tool ${name}` }], isError: true };
      }
    } catch (e) {
      return { content: [{ type: "text", text: `adapter error calling ${name}: ${e.message}` }], isError: true };
    }
  });

  await server.connect(new StdioServerTransport());
  log("connected (stdio). Ready.");
}

main().catch((e) => {
  log("FATAL:", e.stack || e.message);
  process.exit(1);
});
