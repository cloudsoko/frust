import { readFileSync } from "node:fs";
import { callTool, connectMcp, createChecks, login, waitFor } from "./support.mjs";

const checks = createChecks("fidelity");

async function main() {
  const clerk = await login("clerk1", "pw-clerk1");
  const mcp = await connectMcp(clerk.token, "fidelity-clerk1");
  try {
    const tools = (await mcp.client.listTools()).tools;
    const names = new Set(tools.map((tool) => tool.name));
    const meta = await clerk.rest.meta();
    checks.check("every DocType has list and get tools", (meta.json.doctypes ?? []).every((dt) => names.has(`list_${dt.name}`) && names.has(`get_${dt.name}`)));
    checks.check("child DocType itself has tools", names.has("list_mcp_line") && names.has("get_mcp_line"));

    const create = tools.find((tool) => tool.name === "create_expense_claim");
    const fields = create?.inputSchema?.properties ?? {};
    checks.check("parent Currency is a decimal string", fields.amount?.type === "string" && !!fields.amount.pattern);
    checks.check("Link carries target-DocType hint", fields.party?.["x-frust-link-doctype"] === "mcp_party");
    checks.check("Select carries exact enum", JSON.stringify(fields.category?.enum) === JSON.stringify(["Travel", "Meals", "Supplies"]));
    checks.check("Table is a nested child array", fields.lines?.type === "array" && fields.lines?.items?.type === "object" && fields.lines?.["x-frust-child-doctype"] === "mcp_line");
    const child = fields.lines?.items?.properties ?? {};
    checks.check("child Link hint survives nesting", child.item?.["x-frust-link-doctype"] === "mcp_party");
    checks.check("child Select enum survives nesting", child.category?.enum?.includes("Supplies"));
    checks.check("child Currency stays string", child.amount?.type === "string");

    checks.check("expense create/update/submit/delete are exposed", names.has("create_expense_claim") && names.has("update_expense_claim") && names.has("submit_expense_claim") && names.has("delete_expense_claim"));
    checks.check("disabled activity delete is structurally absent", names.has("create_mcp_activity") && !names.has("delete_mcp_activity"));
    checks.check("activity update and submit are structurally absent", !names.has("update_mcp_activity") && !names.has("submit_mcp_activity"));
    checks.check("child writes are structurally absent", !names.has("create_mcp_line") && !names.has("update_mcp_line"));

    const parties = await clerk.rest.read("mcp_party", {});
    const party = parties.json.rows?.[0]?.id;
    const amount = "1234567890.0123456789";
    const created = await callTool(mcp.client, "create_expense_claim", {
      purpose: "MCP fidelity round trip",
      party,
      category: "Supplies",
      amount,
      lines: [{ item: party, category: "Supplies", amount: "0.0100" }],
      workflow_state: "Draft",
    });
    checks.check("nested fidelity create succeeds", !created.isError, JSON.stringify(created.data).slice(0, 180));
    checks.check("parent decimal round-trips as string", typeof created.data.row?.amount === "string" && created.data.row.amount === amount, `value=${created.data.row?.amount}`);
    checks.check("child decimal round-trips as string", typeof created.data.row?.lines?.[0]?.amount === "string" && created.data.row.lines[0].amount === "0.0100", `value=${created.data.row?.lines?.[0]?.amount}`);
    checks.check("Link and Select values round-trip", created.data.row?.party === party && created.data.row?.category === "Supplies" && created.data.row?.lines?.[0]?.item === party);

    const reread = await clerk.rest.read("expense_claim", {
      filter: { path: "id", op: "eq", value: { kind: "record", v: created.data.record } }, limit: 1,
    });
    checks.check("REST reread preserves exact parent decimal string", reread.json.rows?.[0]?.amount === amount && typeof reread.json.rows[0].amount === "string");
    checks.check("REST reread preserves exact child decimal string", reread.json.rows?.[0]?.lines?.[0]?.amount === "0.0100" && typeof reread.json.rows[0].lines[0].amount === "string");

    const numeric = await callTool(mcp.client, "create_expense_claim", {
      purpose: "numeric money must fail", party, category: "Travel", amount: 1.25,
    });
    checks.check("JSON-number Currency is refused before REST write", numeric.isError && /decimal string|JSON number/.test(JSON.stringify(numeric.data)));

    const trace = created.data.trace;
    checks.check("MCP write returns an mcp-prefixed trace id", typeof trace === "string" && trace.startsWith("mcp-"));
    const logPath = process.env.FRUST_KERNEL_LOG;
    if (logPath) {
      const line = await waitFor(() => {
        const text = readFileSync(logPath, "utf8");
        return text.split(/\r?\n/).find((candidate) => candidate.includes(`"trace":"${trace}"`) && candidate.includes("broker_verb"));
      });
      checks.check("kernel telemetry visibly attributes MCP-caused write", !!line, trace);
    } else {
      console.log("SKIP  fidelity: kernel telemetry assertion (FRUST_KERNEL_LOG not set)");
    }
  } finally {
    await mcp.close();
  }
  checks.finish();
}

main().catch((error) => { console.error(error.stack ?? error); process.exit(2); });
