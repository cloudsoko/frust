import { readFileSync } from "node:fs";

const VERBS = new Set(["create", "update", "submit", "delete"]);

function normalizeExposure(raw) {
  const out = new Map();
  for (const [doctype, value] of Object.entries(raw ?? {})) {
    const enabled = new Set();
    if (Array.isArray(value)) {
      for (const verb of value) enabled.add(verb);
    } else if (value && typeof value === "object") {
      for (const [verb, on] of Object.entries(value)) if (on) enabled.add(verb);
    } else {
      throw new Error(`write exposure for '${doctype}' must be an array or object`);
    }
    for (const verb of enabled) {
      if (!VERBS.has(verb)) throw new Error(`unknown write exposure verb '${verb}' for '${doctype}'`);
    }
    out.set(doctype, enabled);
  }
  return out;
}

export function loadConfig(env = process.env) {
  let file = {};
  if (env.FRUST_MCP_CONFIG) {
    file = JSON.parse(readFileSync(env.FRUST_MCP_CONFIG, "utf8"));
  }
  let exposure = file.writes ?? {};
  if (env.FRUST_MCP_WRITE_EXPOSURE) exposure = JSON.parse(env.FRUST_MCP_WRITE_EXPOSURE);
  const port = Number(env.FRUST_MCP_PORT ?? file.port ?? 8796);
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error(`invalid MCP port '${port}'`);
  return {
    base: env.FRUST_BASE ?? file.kernelBase ?? "http://127.0.0.1:8795",
    host: env.FRUST_MCP_HOST ?? file.host ?? "127.0.0.1",
    port,
    pollMs: Number(env.FRUST_MCP_POLL_MS ?? file.pollMs ?? 250),
    writes: normalizeExposure(exposure),
  };
}

export function enabledVerbs(config, doctype) {
  return new Set([...(config.writes.get("*") ?? []), ...(config.writes.get(doctype) ?? [])]);
}
