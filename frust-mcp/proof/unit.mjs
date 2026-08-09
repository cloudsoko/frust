import { strict as assert } from "node:assert";
import { enabledVerbs, loadConfig } from "../src/config.mjs";
import { encodeDoc, objectSchema } from "../src/schema.mjs";

const child = { name: "line", fields: [
  { fieldname: "target", fieldtype: "Link", options: ["party"] },
  { fieldname: "kind", fieldtype: "Select", options: ["A", "B"] },
  { fieldname: "amount", fieldtype: "Currency" },
] };
const parent = { name: "invoice", fields: [
  { fieldname: "lines", fieldtype: "Table", options: ["line"] },
  { fieldname: "total", fieldtype: "Currency", required: true },
] };
const byName = new Map([[child.name, child], [parent.name, parent]]);
const schema = objectSchema(parent, byName);
assert.equal(schema.properties.total.type, "string");
assert.equal(schema.properties.lines.items.properties.target["x-frust-link-doctype"], "party");
assert.deepEqual(schema.properties.lines.items.properties.kind.enum, ["A", "B"]);
assert.deepEqual(encodeDoc("invoice", { total: "1.2300", lines: [{ target: "party:a", kind: "A", amount: "0.10" }] }, byName), {
  total: { kind: "decimal", v: "1.2300" },
  lines: [{ target: { kind: "record", v: "party:a" }, kind: "A", amount: { kind: "decimal", v: "0.10" } }],
});
assert.throws(() => encodeDoc("invoice", { total: 1.23 }, byName), /decimal string/);
const config = loadConfig({ FRUST_MCP_WRITE_EXPOSURE: JSON.stringify({ invoice: ["create"], "*": ["update"] }) });
assert.deepEqual([...enabledVerbs(config, "invoice")].sort(), ["create", "update"]);
assert.throws(() => loadConfig({ FRUST_MCP_WRITE_EXPOSURE: JSON.stringify({ invoice: ["delete"] }) }), /no delete route/);
console.log("PASS  unit: schema recursion, typed values, exposure merge, and delete guard");
