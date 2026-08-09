import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";
import { FrustRest } from "../src/rest.mjs";

export const KERNEL = process.env.FRUST_BASE ?? "http://127.0.0.1:8795";
export const MCP = process.env.FRUST_MCP_URL ?? "http://127.0.0.1:8796/mcp";

export async function login(user, pass) {
  const rest = new FrustRest(KERNEL);
  const who = await rest.login(user, pass);
  return { rest, token: who.token, who };
}

export async function connectMcp(token, name = "frust-mcp-proof") {
  const transport = new StreamableHTTPClientTransport(new URL(MCP), {
    requestInit: { headers: { authorization: `Bearer ${token}` } },
  });
  const client = new Client({ name, version: "1.0.0" }, { capabilities: {} });
  await client.connect(transport);
  return { client, transport, close: () => client.close() };
}

export function toolData(result) {
  const text = result.content?.find((item) => item.type === "text")?.text ?? "{}";
  try { return JSON.parse(text); } catch { return { raw: text }; }
}

export async function callTool(client, name, args = {}) {
  const result = await client.callTool({ name, arguments: args });
  return { result, data: toolData(result), isError: result.isError === true };
}

export function canon(rows) {
  function sort(value) {
    if (Array.isArray(value)) return value.map(sort);
    if (value && typeof value === "object") {
      return Object.fromEntries(Object.keys(value).sort().map((key) => [key, sort(value[key])]));
    }
    return value;
  }
  return JSON.stringify([...rows].sort((a, b) => String(a.id).localeCompare(String(b.id))).map(sort));
}

export function createChecks(label) {
  let passed = 0;
  let failed = 0;
  return {
    check(name, condition, detail = "") {
      condition ? passed++ : failed++;
      console.log(`${condition ? "PASS" : "FAIL"}  ${label}: ${name}${detail ? ` -- ${detail}` : ""}`);
    },
    finish() {
      console.log(`\n${label}: ${passed} passed, ${failed} failed`);
      if (failed) process.exitCode = 1;
      return failed === 0;
    },
  };
}

export async function waitFor(predicate, timeoutMs = 7000, intervalMs = 100) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await predicate();
    if (value) return value;
    await new Promise((resolve) => setTimeout(resolve, intervalMs));
  }
  return null;
}
