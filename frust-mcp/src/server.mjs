#!/usr/bin/env node
import { randomUUID } from "node:crypto";
import { createServer } from "node:http";
import { StreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/streamableHttp.js";
import { isInitializeRequest } from "@modelcontextprotocol/sdk/types.js";
import { createAdapter } from "./adapter.mjs";
import { loadConfig } from "./config.mjs";

const log = (...args) => console.error("[frust-mcp]", ...args);

function bearer(request) {
  const value = request.headers.authorization;
  return typeof value === "string" && value.startsWith("Bearer ") ? value.slice(7) : null;
}

async function jsonBody(request) {
  const chunks = [];
  for await (const chunk of request) chunks.push(chunk);
  if (!chunks.length) return undefined;
  return JSON.parse(Buffer.concat(chunks).toString("utf8"));
}

function reply(response, status, body) {
  const text = typeof body === "string" ? body : JSON.stringify(body);
  response.writeHead(status, { "content-type": typeof body === "string" ? "text/plain" : "application/json" });
  response.end(text);
}

async function main() {
  const config = loadConfig();
  const sessions = new Map();

  const http = createServer(async (request, response) => {
    try {
      const url = new URL(request.url ?? "/", `http://${request.headers.host ?? "localhost"}`);
      if (url.pathname === "/health" && request.method === "GET") return reply(response, 200, { ok: true, sessions: sessions.size });
      if (url.pathname !== "/mcp" || !["GET", "POST", "DELETE"].includes(request.method ?? "")) {
        return reply(response, 404, { error: "not found" });
      }
      const token = bearer(request);
      if (!token) return reply(response, 401, { error: "Authorization: Bearer <Frust session token> is required" });
      const sessionId = request.headers["mcp-session-id"];
      let session = typeof sessionId === "string" ? sessions.get(sessionId) : undefined;
      let body;
      if (request.method === "POST") body = await jsonBody(request);

      if (session) {
        if (session.token !== token) return reply(response, 403, { error: "MCP session belongs to a different Frust principal" });
      } else if (request.method === "POST" && !sessionId && isInitializeRequest(body)) {
        const adapter = await createAdapter({ token, config, log });
        let transport;
        transport = new StreamableHTTPServerTransport({
          sessionIdGenerator: randomUUID,
          onsessioninitialized: (id) => {
            session = { token, transport, adapter };
            sessions.set(id, session);
            log(`session ${id} initialized; ${adapter.doctypes.length} doctypes, ${adapter.tools.length} tools`);
          },
        });
        transport.onclose = () => {
          const id = transport.sessionId;
          if (id) sessions.delete(id);
          void adapter.close();
          log(`session ${id ?? "uninitialized"} closed`);
        };
        await adapter.server.connect(transport);
        await transport.handleRequest(request, response, body);
        return;
      } else {
        return reply(response, sessionId ? 404 : 400, { error: "invalid or missing MCP session" });
      }
      await session.transport.handleRequest(request, response, body);
    } catch (error) {
      log("request failed:", error.stack ?? error.message);
      if (!response.headersSent) reply(response, 500, { error: "internal MCP server error" });
      else response.end();
    }
  });

  http.listen(config.port, config.host, () => {
    log(`streamable HTTP listening on http://${config.host}:${config.port}/mcp; kernel ${config.base}`);
  });

  async function shutdown() {
    http.close();
    await Promise.allSettled([...sessions.values()].map(async (session) => {
      await session.adapter.close();
      await session.transport.close();
    }));
  }
  process.on("SIGINT", () => void shutdown().finally(() => process.exit(0)));
  process.on("SIGTERM", () => void shutdown().finally(() => process.exit(0)));
}

main().catch((error) => {
  log("FATAL:", error.stack ?? error.message);
  process.exit(1);
});
