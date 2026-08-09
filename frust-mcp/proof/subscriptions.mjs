import { ResourceUpdatedNotificationSchema } from "@modelcontextprotocol/sdk/types.js";
import { callTool, canon, connectMcp, createChecks, login, waitFor } from "./support.mjs";

const checks = createChecks("subscriptions");
const EXPENSE = "frust://doctype/expense_claim";
const PRIVATE = "frust://doctype/mcp_private_rollup";

function watch(client) {
  const events = [];
  client.setNotificationHandler(ResourceUpdatedNotificationSchema, (notification) => {
    events.push({ uri: notification.params.uri, at: Date.now() });
  });
  return events;
}

async function main() {
  const [clerk, manager] = await Promise.all([login("clerk1", "pw-clerk1"), login("manager", "pw-manager")]);
  let clerkMcp = await connectMcp(clerk.token, "subscriptions-clerk");
  const managerMcp = await connectMcp(manager.token, "subscriptions-manager");
  const clerkEvents = watch(clerkMcp.client);
  const managerEvents = watch(managerMcp.client);
  try {
    await clerkMcp.client.subscribeResource({ uri: EXPENSE });
    await clerkMcp.client.subscribeResource({ uri: PRIVATE });
    await managerMcp.client.subscribeResource({ uri: PRIVATE });
    await new Promise((resolve) => setTimeout(resolve, 400));

    const party = (await clerk.rest.read("mcp_party", {})).json.rows?.[0]?.id;
    const created = await callTool(clerkMcp.client, "create_expense_claim", {
      purpose: "subscription readable tick", party, category: "Travel", amount: "77.70", lines: [], workflow_state: "Draft",
    });
    checks.check("readable write succeeds", !created.isError);
    const readableTick = await waitFor(() => clerkEvents.find((event) => event.uri === EXPENSE));
    checks.check("client receives updated notification for readable DocType", !!readableTick);

    const privateStartClerk = clerkEvents.length;
    const privateStartManager = managerEvents.length;
    const bucket = `private-${Date.now()}`;
    const activity = await callTool(clerkMcp.client, "create_mcp_activity", { bucket });
    checks.check("rollup-triggering source write succeeds", !activity.isError);
    const managerTick = await waitFor(() => managerEvents.slice(privateStartManager).find((event) => event.uri === PRIVATE));
    checks.check("manager receives tick proving unreadable resource changed", !!managerTick);
    await new Promise((resolve) => setTimeout(resolve, 800));
    checks.check("clerk receives nothing for unreadable DocType", !clerkEvents.slice(privateStartClerk).some((event) => event.uri === PRIVATE));

    const managerResource = await managerMcp.client.readResource({ uri: PRIVATE });
    const managerRows = JSON.parse(managerResource.contents[0].text).rows ?? [];
    checks.check("private tick provenance is a manager-readable rollup row", managerRows.some((row) => row.k === bucket), `rows=${managerRows.length}`);
    const clerkDirect = await clerk.rest.read("mcp_private_rollup", {});
    checks.check("same principal has no REST read door to private rows", !clerkDirect.ok || (clerkDirect.json.rows ?? []).length === 0);

    await clerkMcp.close();
    const direct = await clerk.rest.write("expense_claim", { doc: {
      purpose: "written while MCP disconnected",
      party: { kind: "record", v: party }, category: "Meals",
      amount: { kind: "decimal", v: "88.80" }, lines: [], workflow_state: "Draft",
    } });
    checks.check("row changes while MCP client is disconnected", direct.ok);
    clerkMcp = await connectMcp(clerk.token, "subscriptions-clerk-reconnected");
    const snapshot = await clerkMcp.client.readResource({ uri: EXPENSE });
    const mcpRows = JSON.parse(snapshot.contents[0].text).rows ?? [];
    const restRows = (await clerk.rest.read("expense_claim", {})).json.rows ?? [];
    checks.check("reconnect refetch is byte-equal to REST", canon(mcpRows) === canon(restRows));
    checks.check("reconnect refetch contains disconnected write provenance", mcpRows.some((row) => row.id === direct.json.record && row.owner === "app_user:clerk1"));
  } finally {
    await Promise.allSettled([clerkMcp.close(), managerMcp.close()]);
  }
  checks.finish();
}

main().catch((error) => { console.error(error.stack ?? error); process.exit(2); });
