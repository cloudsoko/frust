// WO-059 containment proof. Drives frust-mcp through the OFFICIAL MCP client
// SDK (a real MCP consumer, stdio transport), one server subprocess per Frust
// principal, and compares its results BYTE-EQUAL against the raw REST surface
// for the same principal. Asserts PROVENANCE (whose rows), never just "data
// came back" (the WO-039 lesson). Predictions are printed before each block.
//
// Run: node proof/containment.mjs   (kernel must be serving on FRUST_BASE)

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
import { FrustRest } from "../src/rest.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SERVER = join(HERE, "..", "src", "server.mjs");
const BASE = process.env.FRUST_BASE || "http://127.0.0.1:8795";
const DT = "expense_claim";

let pass = 0, fail = 0;
const results = [];
function check(name, cond, detail = "") {
  (cond ? pass++ : fail++);
  results.push(`${cond ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}`);
  console.log(`${cond ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}`);
}
function say(s) { console.log(s); }
function predict(s) { console.log("PREDICT: " + s); }

// spawn one frust-mcp server bound to a principal, run fn(client), close.
async function withMcp({ user, pass: pw, token, writes }, fn) {
  const env = { ...process.env, FRUST_BASE: BASE };
  if (token) env.FRUST_TOKEN = token; else { env.FRUST_USER = user; env.FRUST_PASS = pw; }
  env.FRUST_MCP_WRITES = writes ? "on" : "off";
  if (!token) delete env.FRUST_TOKEN;
  const transport = new StdioClientTransport({ command: process.execPath, args: [SERVER], env, stderr: "inherit" });
  const client = new Client({ name: "wo059-proof", version: "0.0.1" }, { capabilities: {} });
  await client.connect(transport);
  try { return await fn(client); } finally { await client.close(); }
}

function toolText(res) {
  const t = res.content?.find((c) => c.type === "text")?.text ?? "";
  try { return JSON.parse(t); } catch { return { _raw: t }; }
}
async function callTool(client, name, args) {
  const res = await client.callTool({ name, arguments: args || {} });
  return { isError: !!res.isError, data: toolText(res), raw: res };
}

// canonical form: sort rows by id, sort object keys — so "byte-equal" ignores
// only key/element ORDER, which the evolution policy explicitly unpromises.
function canon(rows) {
  const sortKeys = (o) =>
    o && typeof o === "object" && !Array.isArray(o)
      ? Object.fromEntries(Object.keys(o).sort().map((k) => [k, sortKeys(o[k])]))
      : Array.isArray(o) ? o.map(sortKeys) : o;
  const sorted = [...rows].sort((a, b) => String(a.id).localeCompare(String(b.id))).map(sortKeys);
  return JSON.stringify(sorted);
}

async function restRead(user, pw, body) {
  const r = new FrustRest(BASE);
  await r.login(user, pw);
  const res = await r.read(DT, body || {});
  return res.json.rows || [];
}

async function main() {
  say(`\n=== WO-059 MCP-native containment proof ===`);
  say(`kernel: ${BASE}   doctype: ${DT}\n`);

  // capture a clerk2-owned row id via the MANAGER (who legitimately sees all)
  const mgrRows = await restRead("manager", "pw-manager", {});
  const clerk2Row = mgrRows.find((r) => r.owner === "app_user:clerk2");
  const clerk1RowFromMgr = mgrRows.find((r) => r.owner === "app_user:clerk1");
  say(`fixture: manager sees ${mgrRows.length} rows; a clerk2 row is ${clerk2Row?.id}; a clerk1 row is ${clerk1RowFromMgr?.id}\n`);

  // ── T1: schema fidelity (criterion 1) ─────────────────────────────────────
  say(`--- T1: /meta -> MCP tool schema fidelity ---`);
  predict("read tools list_/get_ exist; with writes on, create_/update_/transition_ exist; " +
    "amount maps to type:string with a decimal-string money note; required carries [purpose, amount]; " +
    "labels are NOT available (meta omits them) so descriptions are synthesised.");
  await withMcp({ user: "clerk1", pass: "pw-clerk1", writes: true }, async (client) => {
    const { tools } = await client.listTools();
    const names = tools.map((t) => t.name);
    check("T1 read tools present", names.includes(`list_${DT}`) && names.includes(`get_${DT}`), names.join(","));
    check("T1 write tools present (writes on)",
      names.includes(`create_${DT}`) && names.includes(`update_${DT}`) && names.includes(`transition_${DT}`));
    const create = tools.find((t) => t.name === `create_${DT}`);
    const amount = create.inputSchema.properties.amount;
    check("T1 amount is a string", amount.type === "string", `type=${amount.type}`);
    check("T1 amount description carries the money/decimal-string convention",
      /decimal|string/i.test(amount.description) && /float/i.test(amount.description));
    const req = create.inputSchema.required || [];
    check("T1 required carried [purpose, amount]", req.includes("purpose") && req.includes("amount"), `required=${JSON.stringify(req)}`);
  });

  // ── T2: THE containment proof — fourth consumer, byte-equal + provenance ───
  say(`\n--- T2: fourth consumer — MCP read is byte-equal to REST, provenance-correct (criterion 2) ---`);
  predict("clerk1 via MCP returns EXACTLY clerk1's 2 rows, byte-equal to clerk1's REST /read; " +
    "every row.owner == app_user:clerk1; a clerk2 row is NEVER returned to clerk1, even with a filter " +
    "aimed at it or a get by its id; manager via MCP == manager REST (all 5).");
  await withMcp({ user: "clerk1", pass: "pw-clerk1", writes: false }, async (client) => {
    const mcp = (await callTool(client, `list_${DT}`, {})).data.rows || [];
    const rest = await restRead("clerk1", "pw-clerk1", {});
    check("T2 clerk1 MCP == REST (byte-equal)", canon(mcp) === canon(rest),
      `mcp=${mcp.length} rest=${rest.length}`);
    check("T2 clerk1 sees exactly 2 rows", mcp.length === 2, `got ${mcp.length}`);
    const allOwned = mcp.every((r) => r.owner === "app_user:clerk1");
    check("T2 PROVENANCE: every clerk1 MCP row is owned by clerk1", allOwned,
      `owners=${[...new Set(mcp.map((r) => r.owner))].join(",")}`);

    // over-read attempts — the whole thesis
    const byId = await callTool(client, `get_${DT}`, { id: clerk2Row.id });
    const leakedById = byId.data.found === true;
    check("T2 ESCALATION GATE: clerk1 get(clerk2 row id) does NOT return it", !leakedById,
      leakedById ? `LEAKED owner=${byId.data.row?.owner}` : "not found (correct)");
    const filtered = (await callTool(client, `list_${DT}`,
      { filter: { path: "owner", op: "eq", value: { kind: "record", v: "app_user:clerk2" } } })).data.rows || [];
    const leakedByFilter = filtered.some((r) => r.owner === "app_user:clerk2");
    check("T2 ESCALATION GATE: a filter aimed at clerk2 cannot widen clerk1's view", !leakedByFilter && filtered.length === 0,
      `rows=${filtered.length}`);
  });
  await withMcp({ user: "manager", pass: "pw-manager", writes: false }, async (client) => {
    const mcp = (await callTool(client, `list_${DT}`, {})).data.rows || [];
    const rest = await restRead("manager", "pw-manager", {});
    check("T2 manager MCP == REST (byte-equal)", canon(mcp) === canon(rest), `mcp=${mcp.length} rest=${rest.length}`);
    const owners = new Set(mcp.map((r) => r.owner));
    check("T2 manager (role) sees all owners", mcp.length >= 5 && owners.has("app_user:clerk1") && owners.has("app_user:clerk2"),
      `n=${mcp.length} owners=${[...owners].join(",")}`);
  });

  // ── T3: write gating (criterion 3) ────────────────────────────────────────
  say(`\n--- T3: write tools — through the broker, opt-in, typed refusals (criterion 3) ---`);

  // (b) opt-in: OFF by default => tools do not even exist
  predict("with writes off, clerk1's tool list has NO create/update/transition — structural, not a runtime check.");
  await withMcp({ user: "clerk1", pass: "pw-clerk1", writes: false }, async (client) => {
    const names = (await client.listTools()).tools.map((t) => t.name);
    const anyWrite = names.some((n) => /^(create|update|transition)_/.test(n));
    check("T3b writes OFF by default: no write tools registered", !anyWrite, names.join(","));
  });

  // (a) a write flows through the broker; hooks fire; the lattice holds
  predict("clerk1 create -> created, owner clerk1; clerk1 Submit -> workflow advances but docstatus stays 0; " +
    "manager Approve -> docstatus 1 (only the manager crosses the lattice).");
  let createdId;
  await withMcp({ user: "clerk1", pass: "pw-clerk1", writes: true }, async (client) => {
    const c = await callTool(client, `create_${DT}`, { purpose: "MCP taxi", amount: "33.00", workflow_state: "Draft" });
    createdId = c.data.record;
    check("T3a create through MCP succeeds", !c.isError && c.data.action === "created", JSON.stringify(c.data));
    // verify persistence + owner + money-as-string via a fresh REST read
    const row = (await restRead("clerk1", "pw-clerk1", { filter: { path: "id", op: "eq", value: { kind: "record", v: createdId } } }))[0];
    check("T3a created row persisted, owned by clerk1, money stored as decimal", !!row && row.owner === "app_user:clerk1" && row.amount === "33",
      JSON.stringify(row));
    const sub = await callTool(client, `transition_${DT}`, { record: createdId, action: "Submit" });
    check("T3a clerk Submit advances workflow but NOT docstatus (lattice)",
      !sub.isError && sub.data.workflow_state === "Submitted for Approval" && sub.data.docstatus === 0,
      JSON.stringify({ ws: sub.data.workflow_state, ds: sub.data.docstatus }));
  });
  await withMcp({ user: "manager", pass: "pw-manager", writes: true }, async (client) => {
    const appr = await callTool(client, `transition_${DT}`, { record: createdId, action: "Approve" });
    check("T3a manager Approve crosses the lattice to docstatus 1", !appr.isError && appr.data.docstatus === 1,
      JSON.stringify({ ws: appr.data.workflow_state, ds: appr.data.docstatus }));
  });

  // (c) a refused write is a TYPED tool error, never a silent "created"
  predict("clerk1 update of a clerk2 row -> isError, and clerk2's data is UNCHANGED; " +
    "malformed money -> isError; a wrong-state transition -> isError (workflow-denied).");
  await withMcp({ user: "clerk1", pass: "pw-clerk1", writes: true }, async (client) => {
    const before = mgrRows.find((r) => r.id === clerk2Row.id);
    const upd = await callTool(client, `update_${DT}`, { record: clerk2Row.id, purpose: "HACKED-BY-AGENT" });
    check("T3c cross-owner update is a TYPED error (not a silent created)", upd.isError, JSON.stringify(upd.data).slice(0, 200));
    // prove nothing changed — read clerk2's row via the manager
    const after = (await restRead("manager", "pw-manager", { filter: { path: "id", op: "eq", value: { kind: "record", v: clerk2Row.id } } }))[0];
    check("T3c the refused update changed NOTHING (clerk2 row intact)", after && after.purpose === before.purpose && after.purpose !== "HACKED-BY-AGENT",
      `purpose now=${after?.purpose}`);
    const bad = await callTool(client, `create_${DT}`, { purpose: "bad money", amount: "not-a-number" });
    check("T3c malformed money is a TYPED error (not created)", bad.isError, JSON.stringify(bad.data).slice(0, 160));
    const wrong = await callTool(client, `transition_${DT}`, { record: createdId, action: "Submit" });
    check("T3c wrong-state transition on an approved doc is a TYPED error", wrong.isError, JSON.stringify(wrong.data).slice(0, 160));
  });

  // ── summary ──
  say(`\n=== SUMMARY: ${pass} passed, ${fail} failed ===`);
  for (const r of results) if (r.startsWith("FAIL")) say("  " + r);
  process.exit(fail === 0 ? 0 : 1);
}

main().catch((e) => { console.error("HARNESS ERROR:", e.stack || e.message); process.exit(2); });
