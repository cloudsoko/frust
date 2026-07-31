// WO-043 criterion 5: a REAL sales_invoice Submit transition, clicked in a real
// Chromium against the live kernel + Desk, emits a REAL email captured by
// lettre's FileTransport — and the CONTENT is asserted, not merely that a file
// appeared (assert-the-outcome; a send() that returned Ok proves nothing about
// what the approver would actually read).
//
// Prerequisites, all through the running kernel — no recompile, no restart:
//   frust serve  with FRUST_MAIL=file FRUST_MAIL_DIR=<MAILDIR>
//   POST /notification  {doctype: sales_invoice, event: on_transition, action: Submit}
//   app_user:manager has an email address
import { chromium } from "playwright";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const BASE = "http://127.0.0.1:3000";
const KERNEL = "http://127.0.0.1:8790";
const MAILDIR = process.env.FRUST_MAIL_DIR || "./mail-capture";

let failures = 0;
function check(cond, msg) {
  console.log((cond ? "  PASS " : "  FAIL ") + msg);
  if (!cond) failures++;
}

function emlFiles() {
  try {
    return readdirSync(MAILDIR)
      .filter((f) => f.endsWith(".eml"))
      .map((f) => join(MAILDIR, f));
  } catch {
    return [];
  }
}

// The mail worker drains on its own thread every 250 ms, so the email arrives
// AFTER the response — that asynchrony is the feature under test, not a
// nuisance. Bounded wait, and a timeout is a real failure.
async function waitForNewEml(before, timeoutMs = 15000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const now = emlFiles().filter((f) => !before.includes(f));
    if (now.length) return now;
    await new Promise((r) => setTimeout(r, 200));
  }
  return [];
}

async function login(ctx, user, pass) {
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

const before = emlFiles();
console.log(`[setup] maildir ${MAILDIR}, ${before.length} .eml already present`);

const browser = await chromium.launch();
let key, total;
try {
  // ── a real invoice, created and submitted by a real clerk in a real browser ──
  console.log("\n[clerk1] create a sales_invoice and Submit it for approval");
  const clerk = await browser.newContext();
  const page = await login(clerk, "clerk1", "pw-clerk1");
  await page.goto(BASE + "/form/sales_invoice");
  await page.fill('input[name="customer"]', "Northwind Traders");
  await page.fill('input[name="total"]', "0");
  await Promise.all([
    page.waitForLoadState("networkidle"),
    page.click('#doc-form button[type="submit"]'),
  ]);
  key = page.url().split("/").pop();

  await page.click("text=+ Add row");
  await page.waitForSelector('input[name="lines.0.qty"]', { state: "visible" });
  await page.fill('input[name="lines.0.item"]', "Sprocket");
  await page.fill('input[name="lines.0.qty"]', "3");
  await page.fill('input[name="lines.0.rate"]', "12.50");
  await page.fill('input[name="total"]', "37.50");
  await Promise.all([
    page.waitForLoadState("networkidle"),
    page.click('button[value="save"]'),
  ]);

  // no mail yet: the rule fires on the TRANSITION, not on the save
  check(
    (await waitForNewEml(before, 1500)).length === 0,
    "no email on plain save — the rule is scoped to the Submit transition"
  );

  const t0 = Date.now();
  await Promise.all([
    page.waitForLoadState("networkidle"),
    page.click('form[action*="/transition/"] button[value="Submit"]'),
  ]);
  const responseMs = Date.now() - t0;
  const badge = await page.$(".fui-page-head .fui-badge");
  const state = badge ? (await badge.innerText()).trim() : "";
  check(state === "Submitted for Approval", `the transition really happened (state "${state}")`);
  console.log(`  (the Submit response came back in ${responseMs} ms)`);
} finally {
  await browser.close();
}

// ── the email, captured off disk ──
console.log("\n[mail] the approver's notification, captured by FileTransport");
const fresh = await waitForNewEml(before);
check(fresh.length === 1, `exactly one email was delivered (got ${fresh.length})`);

if (fresh.length) {
  const raw = readFileSync(fresh[0], "utf8");
  console.log("  ── captured message (as written) ──");
  console.log(raw.split("\n").map((l) => "  | " + l).join("\n"));

  // lettre encodes the body quoted-printable and wraps at 76 columns with a
  // soft `=` line break. Asserting against the raw file would be asserting on
  // the TRANSPORT ENCODING, not the message — and would pass or fail depending
  // on how long the customer's name happens to be. Undo the soft breaks first.
  const eml = raw.replace(/=\r?\n/g, "");

  check(/^To:.*approver@frust\.local/m.test(eml), "addressed to the APPROVER, resolved from role:manager");
  check(/^From:.*frust@frust\.local/m.test(eml), "sent from the configured FRUST_MAIL_FROM");
  check(/^Subject: Approval needed: Northwind Traders/m.test(eml), "subject interpolated the customer field");
  check(eml.includes(`sales_invoice:${key}`), "body names the actual record that transitioned");
  check(/awaiting your approval/.test(eml), "body is the template's prose, rendered");
  check(/State: Submitted for Approval/.test(eml), "body interpolated the post-transition workflow_state");

  // ── money: the STORED decimal, verbatim (ADR-007 compare-never-compute) ──
  // Asserted against what the kernel says it stored, not against the literal
  // this script typed — so it fails if anything REFORMATS or recomputes the
  // amount on the way into the template, and does not fail merely because
  // SurrealDB normalises decimal scale.
  const res = await fetch(`${KERNEL}/read/sales_invoice`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${process.env.FRUST_TOKEN}` },
    body: "{}",
  });
  const body = await res.json();
  total = (body?.rows ?? []).find((r) => r.id === `sales_invoice:${key}`)?.total;
  if (total === undefined) {
    check(false, `could not read the stored total back to compare against (got ${JSON.stringify(body).slice(0, 200)})`);
  } else {
    check(
      eml.includes(`for ${total} is awaiting`),
      `money rendered as the STORED decimal "${total}", not reformatted`
    );
  }

  // the envelope lettre writes alongside — the recipient list, machine-readable
  const jsons = readdirSync(MAILDIR).filter((f) => f.endsWith(".json"));
  check(jsons.length >= 1, "lettre's own envelope .json was written (no second file transport needed)");
  if (jsons.length) {
    const env = readFileSync(join(MAILDIR, jsons[jsons.length - 1]), "utf8");
    check(env.includes("approver@frust.local"), "the envelope names the approver");
  }
}

// ── the delivery is visible to an operator, not just on disk ──
console.log("\n[outbox] delivery state is data, readable through the kernel");
const tok = process.env.FRUST_TOKEN;
if (tok) {
  const r = await fetch(`${KERNEL}/mail/outbox`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: `Bearer ${tok}` },
    body: "{}",
  });
  const j = await r.json();
  const rows = j?.outbox ?? [];
  const mine = rows.filter((x) => x.record === `sales_invoice:${key}`);
  check(mine.length === 1, `the outbox has this invoice's message (got ${mine.length})`);
  if (mine.length) {
    check(mine[0].status === "sent", `and it is marked sent (status "${mine[0].status}")`);
    check(mine[0].attempts === 1, `delivered on the first attempt (attempts ${mine[0].attempts})`);
  }
}

console.log(`\n=== ${failures === 0 ? "ALL CHECKS PASSED" : failures + " CHECK(S) FAILED"} ===`);
process.exit(failures === 0 ? 0 : 1);
