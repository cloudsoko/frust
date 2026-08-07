// Single DocType proof: a runtime-created Single edits through the Desk's
// direct /single/{doctype} form, persists, and never exposes a list view.
import { chromium } from "playwright";

const BASE = process.env.FRUST_DESK ?? "http://127.0.0.1:3000";
const KERNEL = process.env.FRUST_KERNEL ?? "http://127.0.0.1:8790";

let failures = 0;
function check(cond, msg) {
  console.log((cond ? "  PASS " : "  FAIL ") + msg);
  if (!cond) failures++;
}

async function kernel(path, { token, body } = {}) {
  const res = await fetch(`${KERNEL}${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  let json = null;
  try { json = await res.json(); } catch {}
  return { status: res.status, json };
}

async function loginDesk(ctx, user, pass) {
  const page = await ctx.newPage();
  await page.goto(BASE + "/login");
  await page.fill('input[name="user"]', user);
  await page.fill('input[name="pass"]', pass);
  await Promise.all([
    page.waitForLoadState("networkidle"),
    page.click('button[type="submit"], input[type="submit"]'),
  ]);
  return page;
}

const suffix = `${Date.now()}`.slice(-8);
const doctype = `wo061_single_${suffix}`;
const updated = `Desk persisted ${suffix}`;

const login = await kernel("/login", { body: { user: "manager", pass: "pw-manager" } });
const token = login.json?.token;
check(login.status === 200 && !!token, `manager logged into kernel (status ${login.status})`);

const created = await kernel("/doctype", {
  token,
  body: {
    meta: {
      name: doctype,
      label: "WO061 Single",
      issingle: true,
      fields: [
        { fieldname: "title", fieldtype: "Data" },
        { fieldname: "notes", fieldtype: "Text" },
      ],
    },
  },
});
check(created.status === 200, `runtime Single DocType created through kernel (status ${created.status})`);

const browser = await chromium.launch();
try {
  const ctx = await browser.newContext();
  const page = await loginDesk(ctx, "manager", "pw-manager");

  console.log(`\n[direct form] /single/${doctype}`);
  await page.goto(`${BASE}/single/${doctype}`);
  await page.waitForSelector("#doc-form");
  check(page.url().endsWith(`/single/${doctype}`), `loaded direct Single form (${page.url()})`);
  check(await page.locator('input[name="title"]').count() === 1, "title field rendered on the Single form");

  await page.fill('input[name="title"]', updated);
  await Promise.all([
    page.waitForLoadState("networkidle"),
    page.click('#doc-form button[type="submit"]'),
  ]);
  await page.reload();
  await page.waitForSelector("#doc-form");
  const value = await page.locator('input[name="title"]').inputValue();
  check(value === updated, `saved value persisted after reload (${value})`);

  console.log(`\n[list absence] /list/${doctype}`);
  await page.goto(`${BASE}/list/${doctype}`);
  await page.waitForLoadState("networkidle");
  check(page.url().endsWith(`/single/${doctype}`), `Single list route redirects to the form (${page.url()})`);
  check(await page.locator('table').count() === 0, "no list table is rendered for the Single DocType");
} finally {
  await browser.close();
}

console.log(`\n=== ${failures === 0 ? "ALL CHECKS PASSED" : failures + " CHECK(S) FAILED"} ===`);
process.exit(failures === 0 ? 0 : 1);
