// Permanent multi-principal containment regression. Two clients share one
// streamable-HTTP adapter process while retaining distinct kernel sessions.
import { ResourceUpdatedNotificationSchema } from "@modelcontextprotocol/sdk/types.js";
import { callTool, canon, connectMcp, createChecks, login, waitFor } from "./support.mjs";

const checks = createChecks("containment");
const DT = "expense_claim";

async function main() {
  const [c1, c2, manager] = await Promise.all([
    login("clerk1", "pw-clerk1"),
    login("clerk2", "pw-clerk2"),
    login("manager", "pw-manager"),
  ]);
  const [m1, m2, managerMcp] = await Promise.all([
    connectMcp(c1.token, "containment-clerk1"),
    connectMcp(c2.token, "containment-clerk2"),
    connectMcp(manager.token, "containment-manager"),
  ]);
  const managerEvents = [];
  managerMcp.client.setNotificationHandler(ResourceUpdatedNotificationSchema, (notification) => {
    managerEvents.push(notification.params.uri);
  });
  try {
    const [rest1, rest2, all] = await Promise.all([
      c1.rest.read(DT, {}), c2.rest.read(DT, {}), manager.rest.read(DT, {}),
    ]);
    const [via1, via2] = await Promise.all([
      callTool(m1.client, `list_${DT}`), callTool(m2.client, `list_${DT}`),
    ]);
    const rows1 = via1.data.rows ?? [];
    const rows2 = via2.data.rows ?? [];
    checks.check("clerk1 MCP is byte-equal to clerk1 REST", canon(rows1) === canon(rest1.json.rows ?? []));
    checks.check("clerk2 MCP is byte-equal to clerk2 REST", canon(rows2) === canon(rest2.json.rows ?? []));
    checks.check("clerk1 row provenance", rows1.length >= 2 && rows1.every((row) => row.owner === "app_user:clerk1"), `rows=${rows1.length}`);
    checks.check("clerk2 row provenance", rows2.length >= 2 && rows2.every((row) => row.owner === "app_user:clerk2"), `rows=${rows2.length}`);

    const clerk2Row = (all.json.rows ?? []).find((row) => row.owner === "app_user:clerk2");
    const clerk1Row = (all.json.rows ?? []).find((row) => row.owner === "app_user:clerk1");
    checks.check("fixture has both owner partitions", !!clerk1Row && !!clerk2Row);
    const crossGet = await callTool(m1.client, `get_${DT}`, { id: clerk2Row.id });
    checks.check("cross-principal get returns no row", crossGet.data.found === false);
    const crossFilter = await callTool(m1.client, `list_${DT}`, {
      filter: { path: "owner", op: "eq", value: { kind: "record", v: "app_user:clerk2" } },
    });
    checks.check("cross-principal filter is empty", (crossFilter.data.rows ?? []).length === 0);

    const before = clerk2Row.purpose;
    const crossUpdate = await callTool(m1.client, `update_${DT}`, { record: clerk2Row.id, purpose: "cross-owner mutation" });
    checks.check("cross-principal update is refused", crossUpdate.isError);
    const after = await manager.rest.read(DT, {
      filter: { path: "id", op: "eq", value: { kind: "record", v: clerk2Row.id } }, limit: 1,
    });
    checks.check("refused update preserves clerk2 provenance and bytes", after.json.rows?.[0]?.owner === "app_user:clerk2" && after.json.rows[0].purpose === before);

    const crossDelete = await callTool(m1.client, `delete_${DT}`, { record: clerk2Row.id });
    checks.check(
      "cross-principal delete is a typed permission refusal",
      crossDelete.isError
        && crossDelete.data.error?.kind === "permission-denied"
        && /E_DELETE_NO_ROWS/.test(crossDelete.data.error?.detail ?? ""),
      JSON.stringify(crossDelete.data),
    );
    const survived = await manager.rest.read(DT, {
      filter: { path: "id", op: "eq", value: { kind: "record", v: clerk2Row.id } }, limit: 1,
    });
    checks.check(
      "refused delete leaves the other owner's row and provenance intact",
      survived.json.rows?.[0]?.owner === "app_user:clerk2" && canon(survived.json.rows) === canon([clerk2Row]),
    );

    const recordUri = `frust://doctype/${DT}/${encodeURIComponent(clerk2Row.id)}`;
    await managerMcp.client.subscribeResource({ uri: recordUri });
    const deleted = await callTool(managerMcp.client, `delete_${DT}`, { record: clerk2Row.id });
    checks.check("manager deletes the draft through MCP", !deleted.isError && deleted.data.action === "deleted" && deleted.data.id === clerk2Row.id);
    const deleteTick = await waitFor(() => managerEvents.find((uri) => uri === recordUri));
    checks.check("manager receives the deleted resource invalidation tick", !!deleteTick);
    const invalidated = await managerMcp.client.readResource({ uri: recordUri });
    const invalidatedData = JSON.parse(invalidated.contents[0].text);
    checks.check("refetch after delete proves the resource is gone", invalidatedData.found === false && invalidatedData.row === null);

    const reverseGet = await callTool(m2.client, `get_${DT}`, { id: clerk1Row.id });
    checks.check("reverse cross-principal get returns no row", reverseGet.data.found === false);
    checks.check("one server holds all live MCP sessions", (await fetch("http://127.0.0.1:8796/health").then((r) => r.json())).sessions >= 3);
  } finally {
    await Promise.allSettled([m1.close(), m2.close(), managerMcp.close()]);
  }
  checks.finish();
}

main().catch((error) => { console.error(error.stack ?? error); process.exit(2); });
