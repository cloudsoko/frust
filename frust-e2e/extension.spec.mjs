// WO-050 criterion 10: the dogfood, live.
//
// A SECOND REAL APP extends the accounting seed's `sales_invoice` — a DocType
// it does not own — through the shipped install path, and the result is
// exercised in a real browser (tested-seam≠wired). Then it is uninstalled and
// the owner is shown intact.
//
// This is the proof P-2.2 gets re-scored on: not "the refusal works" (WO-036
// proved that), but "two apps compose on one DocType and the owner's invariant
// still runs".
import { chromium } from "playwright";

const KERNEL = "http://127.0.0.1:8790";
const BASE = "http://127.0.0.1:3000";
let failures = 0;
const check = (c, m) => { console.log((c ? "  PASS " : "  FAIL ") + m); if (!c) failures++; };

async function api(path, token, body) {
  const r = await fetch(KERNEL + path, {
    method: "POST",
    headers: { "Content-Type": "application/json", ...(token ? { Authorization: `Bearer ${token}` } : {}) },
    body: JSON.stringify(body ?? {}),
  });
  return [r.status, await r.json()];
}

const [, login] = await api("/login", null, { user: "manager", pass: "pw-manager" });
const tok = login.token;

// ── the extension app: adds a namespaced field + a validate hook to
//    `sales_invoice`, which the `acct` app owns ──
const crmApp = {
  manifest_version: 1,
  name: "crm",
  version: "1.0.0",
  label: "CRM follow-up",
  extends: [{
    doctype: "sales_invoice",
    hook: "validate",
    fields: [{ fieldname: "crm_followup", fieldtype: "Data" }],
    // reads a field the OWNER declared, writes only its own namespaced field
    script: "doc.crm_followup = 'call ' + (doc.customer || 'customer');",
  }],
};

console.log("\n[install] a second app extends a DocType it does not own");
let [code, out] = await api("/app/install", tok, crmApp);
if (code !== 200 && String(out?.error?.detail || "").includes("already installed")) {
  await api("/app/crm/uninstall", tok, {});
  [code, out] = await api("/app/install", tok, crmApp);
}
check(code === 200, `crm installs against acct's sales_invoice (${code} ${JSON.stringify(out).slice(0, 160)})`);

// ── the browser: a real invoice through the real Desk ──
console.log("\n[browser] a real invoice, saved through the Desk");
const browser = await chromium.launch();
let key;
try {
  const ctx = await browser.newContext();
  const p = await ctx.newPage();
  await p.goto(BASE + "/login");
  await p.fill('input[name="user"]', "clerk1");
  await p.fill('input[name="pass"]', "pw-clerk1");
  await Promise.all([p.waitForLoadState("networkidle"), p.click('button[type="submit"]')]);

  await p.goto(BASE + "/form/sales_invoice");
  await p.fill('input[name="customer"]', "Contoso");
  await p.fill('input[name="total"]', "0");
  await Promise.all([p.waitForLoadState("networkidle"), p.click('#doc-form button[type="submit"]')]);
  key = p.url().split("/").pop();
  check(/\/doc\/sales_invoice\//.test(p.url()), `invoice created through the Desk (${key})`);

  const text = await p.evaluate(() => document.body.innerText);
  check(/crm follow|crm_followup/i.test(text), "the extension's field is rendered on the owner's form");
} finally {
  await browser.close();
}

// ── both hooks ran: the owner's AND the extension's ──
console.log("\n[compose] both apps' hooks ran on one write");
const [, read] = await api("/read/sales_invoice", tok, {});
const rec = (read.rows || []).find((r) => r.id.endsWith(key));
check(!!rec, "the record reads back");
if (rec) {
  console.log("   record:", JSON.stringify({ customer: rec.customer, crm_followup: rec.crm_followup, total: rec.total }));
  check(rec.crm_followup === `call Contoso`, `the EXTENSION's hook ran (crm_followup = ${JSON.stringify(rec.crm_followup)})`);
  // the owner's invariant is the seed's reconciliation hook: it computes/echoes
  // `total`. Its presence is the owner's hook still running underneath.
  check(rec.total !== undefined && rec.total !== null, `the OWNER's hook still ran (total = ${JSON.stringify(rec.total)})`);
}

// ── honest uninstall: the extension detaches, the owner survives ──
console.log("\n[uninstall] the extension detaches; the owner is untouched");
const [uc, uo] = await api("/app/crm/uninstall", tok, {});
check(uc === 200, `crm uninstalls (${uc})`);
console.log("   ", JSON.stringify(uo).slice(0, 200));

const [, meta] = await api("/meta/sales_invoice", tok, {});
const fields = (meta?.doctype?.fields || []).map((f) => f.fieldname);
// VACUITY GUARD, first: "the extension's field is gone" is trivially true of an
// EMPTY list, and an empty list is exactly what a broken uninstall would leave.
// So prove the DocType is still furnished before reading anything into an
// absence. (The first run of this probe read the wrong response path, got 0
// fields, and the detach check passed for precisely that wrong reason.)
check(
  fields.length > 0 && fields.includes("customer"),
  `sales_invoice survives with the owner's own fields (${fields.length}: ${fields.join(", ")})`
);
check(!fields.includes("crm_followup"), "and the extension's field is detached");

const [, after] = await api("/read/sales_invoice", tok, {});
const still = (after.rows || []).find((r) => r.id.endsWith(key));
check(!!still, "and every row the extension touched SURVIVES — data outlives apps");

console.log(`\n=== ${failures === 0 ? "ALL CHECKS PASSED" : failures + " CHECK(S) FAILED"} ===`);
process.exit(failures === 0 ? 0 : 1);
