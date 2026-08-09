import { Server } from "@modelcontextprotocol/sdk/server/index.js";
import {
  CallToolRequestSchema,
  ErrorCode,
  ListResourcesRequestSchema,
  ListResourceTemplatesRequestSchema,
  ListToolsRequestSchema,
  McpError,
  ReadResourceRequestSchema,
  SubscribeRequestSchema,
  UnsubscribeRequestSchema,
} from "@modelcontextprotocol/sdk/types.js";
import { enabledVerbs } from "./config.mjs";
import { FrustRest, bareKey } from "./rest.mjs";
import { assertWireDoc, assertWireRows, encodeDoc, objectSchema } from "./schema.mjs";

const COLLECTION_HOST = "doctype";

function collectionUri(doctype) {
  return `frust://${COLLECTION_HOST}/${encodeURIComponent(doctype)}`;
}

function parseResourceUri(raw) {
  let uri;
  try { uri = new URL(raw); } catch { throw new McpError(ErrorCode.InvalidParams, `invalid resource URI '${raw}'`); }
  if (uri.protocol !== "frust:" || uri.hostname !== COLLECTION_HOST) {
    throw new McpError(ErrorCode.InvalidParams, `unsupported resource URI '${raw}'`);
  }
  const parts = uri.pathname.split("/").filter(Boolean).map(decodeURIComponent);
  if (parts.length < 1 || parts.length > 2) throw new McpError(ErrorCode.InvalidParams, `unsupported resource URI '${raw}'`);
  return { doctype: parts[0], id: parts[1] };
}

function ok(json) {
  return { content: [{ type: "text", text: JSON.stringify(json, null, 2) }] };
}

function refused(response, note) {
  return {
    content: [{
      type: "text",
      text: `${note}\nThe kernel refused (HTTP ${response.status}); the operation did not happen.\n${JSON.stringify(response.json, null, 2)}`,
    }],
    isError: true,
  };
}

function listInputSchema() {
  const filter = {
    type: "object",
    description: "Structured Frust filter. Filters can only narrow the kernel permission-filtered result.",
    properties: {
      path: { oneOf: [{ type: "string" }, { type: "array", items: { type: "string" } }] },
      op: { type: "string", enum: ["eq", "ne", "gt", "gte", "lt", "lte", "inside", "contains"] },
      value: {},
      and: { type: "array", items: { type: "object" } },
      or: { type: "array", items: { type: "object" } },
      not: { type: "object" },
    },
    additionalProperties: false,
  };
  return {
    type: "object",
    properties: {
      filter,
      fields: { type: "array", items: { type: "string" } },
      order: {
        type: "object",
        properties: { path: { type: "string" }, dir: { type: "string", enum: ["asc", "desc"] } },
        required: ["path"],
        additionalProperties: false,
      },
      limit: { type: "integer", minimum: 0 },
      start: { type: "integer", minimum: 0 },
    },
    additionalProperties: false,
  };
}

export function toolsForDoctype(doctype, byName, verbs) {
  const name = doctype.name;
  const tools = [
    {
      name: `list_${name}`,
      description: `List permission-filtered ${name} records through the Frust read door.`,
      inputSchema: listInputSchema(),
    },
    {
      name: `get_${name}`,
      description: `Get one visible ${name} record by full id or bare key.`,
      inputSchema: {
        type: "object",
        properties: { id: { type: "string" } },
        required: ["id"],
        additionalProperties: false,
      },
    },
  ];
  const docSchema = objectSchema(doctype, byName);
  if (verbs.has("create")) {
    tools.push({
      name: `create_${name}`,
      description: `Create ${name} through the Frust broker. Currency inputs are decimal strings; child tables are nested arrays.`,
      inputSchema: docSchema,
    });
  }
  if (verbs.has("update")) {
    tools.push({
      name: `update_${name}`,
      description: `Partially update a visible ${name} through the Frust broker.`,
      inputSchema: {
        ...structuredClone(docSchema),
        properties: { record: { type: "string" }, ...structuredClone(docSchema.properties) },
        required: ["record"],
      },
    });
  }
  if (verbs.has("submit") && doctype.submittable) {
    tools.push({
      name: `submit_${name}`,
      description: `Apply a workflow action to ${name}; action defaults to Submit. The kernel enforces state, role, and docstatus.`,
      inputSchema: {
        type: "object",
        properties: { record: { type: "string" }, action: { type: "string", default: "Submit" } },
        required: ["record"],
        additionalProperties: false,
      },
    });
  }
  return tools;
}

export async function createAdapter({ token, config, log = () => {} }) {
  const rest = new FrustRest(config.base, token);
  const metadata = await rest.meta();
  if (!metadata.ok) throw new Error(`GET /meta failed (HTTP ${metadata.status}): ${JSON.stringify(metadata.json)}`);
  const doctypes = metadata.json.doctypes ?? [];
  const byName = new Map(doctypes.map((doctype) => [doctype.name, doctype]));
  const toolMap = new Map();
  for (const doctype of doctypes) {
    for (const tool of toolsForDoctype(doctype, byName, enabledVerbs(config, doctype.name))) toolMap.set(tool.name, tool);
  }

  const server = new Server(
    { name: "frust-mcp", version: "1.0.0" },
    { capabilities: { tools: {}, resources: { subscribe: true, listChanged: false } } },
  );
  const resourceSubscriptions = new Map();
  const liveByDoctype = new Map();
  let closed = false;

  server.setRequestHandler(ListToolsRequestSchema, async () => ({ tools: [...toolMap.values()] }));
  server.setRequestHandler(CallToolRequestSchema, async (request) => {
    const name = request.params.name;
    if (!toolMap.has(name)) return { ...ok({ error: `unknown or disabled tool '${name}'`, structural: true, available: false }), isError: true };
    const args = request.params.arguments ?? {};
    const split = name.indexOf("_");
    const verb = name.slice(0, split);
    const doctype = name.slice(split + 1);
    try {
      if (verb === "list") {
        const body = {};
        for (const key of ["filter", "fields", "order", "limit", "start"]) if (args[key] !== undefined) body[key] = args[key];
        const response = await rest.read(doctype, body);
        if (!response.ok) return refused(response, `${name} failed.`);
        assertWireRows(doctype, response.json.rows ?? [], byName);
        return ok(response.json);
      }
      if (verb === "get") {
        const fullId = String(args.id).includes(":") ? String(args.id) : `${doctype}:${args.id}`;
        const response = await rest.read(doctype, {
          filter: { path: "id", op: "eq", value: { kind: "record", v: fullId } }, limit: 1,
        });
        if (!response.ok) return refused(response, `${name} failed.`);
        const row = (response.json.rows ?? [])[0];
        if (!row) return ok({ found: false });
        assertWireDoc(doctype, row, byName);
        return ok({ found: true, row });
      }
      if (verb === "create") {
        const response = await rest.write(doctype, { doc: encodeDoc(doctype, args, byName) });
        if (!response.ok) return refused(response, `${name} refused.`);
        assertWireDoc(doctype, response.json.created, byName);
        return ok({ action: response.json.action, record: response.json.record, row: response.json.created, trace: response.traceId });
      }
      if (verb === "update") {
        const { record, ...changes } = args;
        const response = await rest.write(doctype, { record: bareKey(record), doc: encodeDoc(doctype, changes, byName) });
        if (!response.ok) return refused(response, `${name} refused.`);
        assertWireDoc(doctype, response.json.created, byName);
        return ok({ action: response.json.action, record: response.json.record, row: response.json.created, trace: response.traceId });
      }
      if (verb === "submit") {
        const response = await rest.transition(doctype, bareKey(args.record), args.action ?? "Submit");
        return response.ok ? ok({ ...response.json, trace: response.traceId }) : refused(response, `${name} refused.`);
      }
      return { ...ok({ error: `unknown tool '${name}'` }), isError: true };
    } catch (error) {
      return { ...ok({ error: error.message, tool: name }), isError: true };
    }
  });

  server.setRequestHandler(ListResourcesRequestSchema, async () => ({
    resources: doctypes.filter((doctype) => doctype.can_read !== false).map((doctype) => ({
      uri: collectionUri(doctype.name),
      name: doctype.name,
      title: doctype.label ?? doctype.name,
      description: `Current permission-filtered ${doctype.name} collection. Read again after an updated notification.`,
      mimeType: "application/json",
    })),
  }));
  server.setRequestHandler(ListResourceTemplatesRequestSchema, async () => ({
    resourceTemplates: [{
      uriTemplate: "frust://doctype/{doctype}/{id}",
      name: "frust-record",
      description: "One Frust record. The id may be a full record id or bare key.",
      mimeType: "application/json",
    }],
  }));
  server.setRequestHandler(ReadResourceRequestSchema, async (request) => {
    const parsed = parseResourceUri(request.params.uri);
    if (!byName.has(parsed.doctype)) throw new McpError(ErrorCode.InvalidParams, `unknown DocType '${parsed.doctype}'`);
    let response;
    if (parsed.id) {
      const fullId = parsed.id.includes(":") ? parsed.id : `${parsed.doctype}:${parsed.id}`;
      response = await rest.read(parsed.doctype, {
        filter: { path: "id", op: "eq", value: { kind: "record", v: fullId } }, limit: 1,
      });
      if (!response.ok) throw new McpError(ErrorCode.InvalidRequest, `kernel refused resource read (HTTP ${response.status})`);
      const row = (response.json.rows ?? [])[0] ?? null;
      if (row) assertWireDoc(parsed.doctype, row, byName);
      return { contents: [{ uri: request.params.uri, mimeType: "application/json", text: JSON.stringify({ found: !!row, row }) }] };
    }
    response = await rest.read(parsed.doctype, {});
    if (!response.ok) throw new McpError(ErrorCode.InvalidRequest, `kernel refused resource read (HTTP ${response.status})`);
    assertWireRows(parsed.doctype, response.json.rows ?? [], byName);
    return { contents: [{ uri: request.params.uri, mimeType: "application/json", text: JSON.stringify(response.json) }] };
  });

  async function pollLive(entry) {
    if (closed || entry.polling) return;
    entry.polling = true;
    try {
      const response = await rest.events(entry.sub);
      if (!response.ok) throw new Error(`events/${entry.sub} HTTP ${response.status}`);
      if (response.json.alive === false) {
        const replacement = await rest.subscribe(entry.doctype);
        if (!replacement.ok) throw new Error(`resubscribe ${entry.doctype} HTTP ${replacement.status}`);
        entry.sub = replacement.json.sub;
        for (const uri of entry.uris) await server.sendResourceUpdated({ uri });
        log(`kernel subscription reconnected for ${entry.doctype}; refetch notifications sent`);
      } else {
        for (const tick of response.json.events ?? []) {
          for (const uri of entry.uris) {
            const parsed = resourceSubscriptions.get(uri);
            if (parsed && (!parsed.id || parsed.id === tick.id || `${parsed.doctype}:${parsed.id}` === tick.id)) {
              await server.sendResourceUpdated({ uri });
            }
          }
        }
      }
    } catch (error) {
      log(`subscription poll failed for ${entry.doctype}: ${error.message}`);
    } finally {
      entry.polling = false;
    }
  }

  server.setRequestHandler(SubscribeRequestSchema, async (request) => {
    const parsed = parseResourceUri(request.params.uri);
    if (!byName.has(parsed.doctype)) throw new McpError(ErrorCode.InvalidParams, `unknown DocType '${parsed.doctype}'`);
    if (resourceSubscriptions.has(request.params.uri)) return {};
    let entry = liveByDoctype.get(parsed.doctype);
    if (!entry) {
      const response = await rest.subscribe(parsed.doctype);
      if (!response.ok) throw new McpError(ErrorCode.InvalidRequest, `kernel subscribe refused (HTTP ${response.status})`);
      entry = { doctype: parsed.doctype, sub: response.json.sub, uris: new Set(), polling: false, timer: null };
      entry.timer = setInterval(() => void pollLive(entry), config.pollMs);
      entry.timer.unref?.();
      liveByDoctype.set(parsed.doctype, entry);
    }
    resourceSubscriptions.set(request.params.uri, parsed);
    entry.uris.add(request.params.uri);
    return {};
  });
  server.setRequestHandler(UnsubscribeRequestSchema, async (request) => {
    const parsed = resourceSubscriptions.get(request.params.uri);
    if (!parsed) return {};
    resourceSubscriptions.delete(request.params.uri);
    const entry = liveByDoctype.get(parsed.doctype);
    entry?.uris.delete(request.params.uri);
    if (entry && entry.uris.size === 0) {
      clearInterval(entry.timer);
      liveByDoctype.delete(parsed.doctype);
      await rest.unsubscribe(entry.sub);
    }
    return {};
  });

  async function close() {
    if (closed) return;
    closed = true;
    const entries = [...liveByDoctype.values()];
    for (const entry of entries) clearInterval(entry.timer);
    await Promise.allSettled(entries.map((entry) => rest.unsubscribe(entry.sub)));
    liveByDoctype.clear();
    resourceSubscriptions.clear();
  }

  return { server, rest, doctypes, tools: [...toolMap.values()], close };
}

export { collectionUri, parseResourceUri };
