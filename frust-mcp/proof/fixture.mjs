import { FrustRest } from "../src/rest.mjs";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const BASE = process.env.FRUST_BASE ?? "http://127.0.0.1:8795";

async function required(response, label) {
  if (!response.ok) throw new Error(`${label} failed (HTTP ${response.status}): ${JSON.stringify(response.json)}`);
  return response.json;
}

async function create(rest, doctype, doc) {
  const json = await required(await rest.write(doctype, { doc }), `create ${doctype}`);
  return json.record;
}

export async function setupFixture() {
  const manager = new FrustRest(BASE);
  await manager.login("manager", "pw-manager");
  await required(await manager.call("POST", "/app/install", { body: {
    manifest_version: 1,
    name: "mcp_full",
    version: "1.0.0",
    doctypes: [
      { name: "mcp_party", fields: [
        { fieldname: "party_name", fieldtype: "Data", required: true },
      ] },
      { name: "mcp_line", fields: [
        { fieldname: "item", fieldtype: "Link", options: ["mcp_party"], required: true },
        { fieldname: "category", fieldtype: "Select", options: ["Travel", "Meals", "Supplies"], required: true },
        { fieldname: "amount", fieldtype: "Currency", required: true },
      ] },
      { name: "expense_claim", submittable: true, fields: [
        { fieldname: "purpose", fieldtype: "Data", required: true },
        { fieldname: "party", fieldtype: "Link", options: ["mcp_party"], required: true },
        { fieldname: "category", fieldtype: "Select", options: ["Travel", "Meals", "Supplies"], required: true },
        { fieldname: "amount", fieldtype: "Currency", required: true },
        { fieldname: "lines", fieldtype: "Table", options: ["mcp_line"] },
        { fieldname: "workflow_state", fieldtype: "Data" },
      ] },
      { name: "mcp_activity", fields: [
        { fieldname: "bucket", fieldtype: "Data", required: true },
      ], aggregates: [{ kind: "counter", rollup: "mcp_private_rollup", key: "bucket", metrics: [] }] },
      { name: "mcp_private_rollup", fields: [
        { fieldname: "k", fieldtype: "Data" },
        { fieldname: "n", fieldtype: "Data" },
      ] },
    ],
    workflows: [{
      name: "expense_approval",
      doctype: "expense_claim",
      states: [
        { name: "Draft", docstatus: 0 },
        { name: "Submitted for Approval", docstatus: 0 },
        { name: "Approved", docstatus: 1 },
      ],
      transitions: [
        { from: "Draft", to: "Submitted for Approval", role: "clerk", action: "Submit" },
        { from: "Submitted for Approval", to: "Approved", role: "manager", action: "Approve" },
      ],
    }],
  } }), "install mcp_full app");

  const principals = [
    ["clerk1", "pw-clerk1"],
    ["clerk2", "pw-clerk2"],
    ["manager", "pw-manager"],
  ];
  for (const [user, pass] of principals) {
    const rest = new FrustRest(BASE);
    await rest.login(user, pass);
    const party = await create(rest, "mcp_party", { party_name: `${user} party` });
    const amounts = user === "manager" ? ["999.00"] : user === "clerk1" ? ["42.00", "118.50"] : ["560.00", "205.00"];
    for (let i = 0; i < amounts.length; i++) {
      await create(rest, "expense_claim", {
        purpose: `${user} fixture ${i + 1}`,
        party: { kind: "record", v: party },
        category: i % 2 ? "Meals" : "Travel",
        amount: { kind: "decimal", v: amounts[i] },
        lines: [{
          item: { kind: "record", v: party },
          category: i % 2 ? "Meals" : "Travel",
          amount: { kind: "decimal", v: amounts[i] },
        }],
        workflow_state: "Draft",
      });
    }
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  setupFixture().then(() => console.log("fixture ready")).catch((error) => { console.error(error); process.exit(1); });
}
