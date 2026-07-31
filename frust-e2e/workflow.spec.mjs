// WO-031 mandatory browser proof: the seed's approval flow, clicked end to end
// through a real Chromium against the live kernel+Desk. Not a broker test.
import { chromium } from "playwright";

const BASE = "http://127.0.0.1:3000";
let failures = 0;
function check(cond, msg) {
  console.log((cond ? "  PASS " : "  FAIL ") + msg);
  if (!cond) failures++;
}

async function login(ctx, user, pass) {
  const page = await ctx.newPage();
  await page.goto(BASE + "/login");
  await page.fill('input[name="user"]', user);
  await page.fill('input[name="pass"]', pass);
  await Promise.all([page.waitForLoadState("networkidle"), page.click('button[type="submit"], input[type="submit"]')]);
  return page;
}

// visible workflow buttons = the transition form's buttons (name=action)
async function actionButtons(page) {
  return await page.$$eval('form[action*="/transition/"] button[name="action"]',
    els => els.map(e => e.getAttribute("value")));
}
async function badge(page) {
  // The state chip beside the <h1>. WO-042 re-skinned the page header, moving
  // the chip from INSIDE the <h1> to a sibling inside `.fui-page-head` — so the
  // old `h1 span` selector read "" for every state and six checks failed while
  // every transition still worked. Deliberately NOT falling back to `h1 span`:
  // a selector that silently matches two generations of markup cannot tell you
  // the next re-skin moved it.
  const el = await page.$(".fui-page-head .fui-badge");
  return el ? (await el.innerText()).trim() : "";
}

const browser = await chromium.launch();
try {
  // ── 1. CLERK creates an invoice with a line ──
  // Two steps, both in-browser: the /form makes the header (the Table field's
  // rich editor is WO-015's /doc view), then the /doc line editor adds the
  // line. An empty header balances (0 == 0); the line makes it 20.00.
  console.log("\n[clerk1] create invoice (header on /form, line on /doc)");
  const clerk = await browser.newContext();
  let page = await login(clerk, "clerk1", "pw-clerk1");
  await page.goto(BASE + "/form/sales_invoice");
  await page.fill('input[name="customer"]', "ACME Corp");
  await page.fill('input[name="total"]', "0");
  await Promise.all([page.waitForLoadState("networkidle"), page.click('#doc-form button[type="submit"]')]);
  const url = page.url();
  const key = url.split("/").pop();
  check(/\/doc\/sales_invoice\//.test(url), `header created, landed on the doc page (${url})`);

  // add the line via the /doc editor, set the balanced total, save
  await page.click("text=+ Add row");
  await page.waitForSelector('input[name="lines.0.qty"]', { state: "visible" });
  await page.fill('input[name="lines.0.item"]', "Widget");
  await page.fill('input[name="lines.0.qty"]', "2");
  await page.fill('input[name="lines.0.rate"]', "10.00");
  await page.fill('input[name="total"]', "20.00");
  await Promise.all([page.waitForLoadState("networkidle"), page.click('button[value="save"]')]);
  const balanced = await page.evaluate(() => document.body.innerText);
  check(!/does not balance|Rejected by/.test(balanced), "line saved and reconciled (2 × 10.00 = 20.00)");

  // ── criterion 1 + 5: buttons render for (state, role); dirty guard coexists ──
  console.log("\n[clerk1] Draft invoice — role-filtered buttons + dirty guard");
  check((await badge(page)) === "Draft", `badge shows workflow state "Draft" (got "${await badge(page)}")`);
  let btns = await actionButtons(page);
  check(btns.includes("Submit"), `clerk sees "Submit" (got [${btns}])`);
  check(!btns.includes("Approve"), `clerk does NOT see "Approve" — role filtering (got [${btns}])`);
  check(!(await page.$('button[value="submit"]')), "raw docstatus Submit is suppressed under workflow");

  // criterion 5: the WO-014 dirty guard still fires with a transition button present
  const dirtyBefore = await page.evaluate(() => window.__frustDirty);
  await page.fill('input[name="customer"]', "ACME Corporation");
  const dirtyAfter = await page.evaluate(() => window.__frustDirty);
  const noteShown = await page.evaluate(() => {
    const n = document.getElementById("dirty-note");
    return n && getComputedStyle(n).display !== "none";
  });
  check(dirtyBefore === false && dirtyAfter === true && noteShown,
    "dirty guard: typing marks dirty and shows the banner, with the workflow button present");
  await page.reload(); // drop the unsaved edit before transitioning

  // ── criterion 2 + 4: clerk submits for approval (0→0) ──
  console.log("\n[clerk1] Submit for Approval");
  await Promise.all([page.waitForLoadState("networkidle"),
    page.click('form[action*="/transition/"] button[value="Submit"]')]);
  check((await badge(page)) === "Submitted for Approval",
    `state advanced to "Submitted for Approval" (got "${await badge(page)}")`);
  btns = await actionButtons(page);
  check(btns.length === 0, `clerk has no further action from this state (got [${btns}])`);

  // ── criterion 3: a role-denied transition surfaces the typed refusal as prose ──
  // From "Submitted for Approval" the clerk CAN name a real action (Approve)
  // that is manager-only — so the judge returns ROLE_DENIED, naming the role
  // that would work. (From Draft it would be WRONG_STATE instead.)
  console.log("\n[clerk1] a manager-only transition is refused, naming the role");
  await clerk.request.post(`${BASE}/transition/sales_invoice/${key}`, {
    form: { action: "Approve" }, maxRedirects: 0, failOnStatusCode: false,
  });
  await page.goto(`${BASE}/doc/sales_invoice/${key}`);
  const flash = await page.evaluate(() => document.body.innerText);
  check(/requires role manager/.test(flash) && /you are 'clerk'/.test(flash),
    "clerk's 'Approve' is refused, naming the role that would work (E_WORKFLOW ROLE_DENIED surfaced as prose)");
  check(!/E_WORKFLOW|at :\d+:\d+|Traceback/.test(flash), "the machine code / trace is stripped from the message");
  check((await badge(page)) === "Submitted for Approval", "the refused transition changed nothing");

  // ── criterion 1 + 4: MANAGER sees Approve/Reject and approves (0→1) ──
  console.log("\n[manager] sees Approve/Reject, approves");
  const mgr = await browser.newContext();
  let mpage = await login(mgr, "manager", "pw-manager");
  await mpage.goto(`${BASE}/doc/sales_invoice/${key}`);
  btns = await actionButtons(mpage);
  check(btns.includes("Approve") && btns.includes("Reject"),
    `manager sees both transitions (got [${btns}])`);
  await Promise.all([mpage.waitForLoadState("networkidle"),
    mpage.click('form[action*="/transition/"] button[value="Approve"]')]);
  check((await badge(mpage)) === "Approved", `state is "Approved" (got "${await badge(mpage)}")`);
  // criterion 2: Approve is the lattice submit — docstatus 0→1, verified in the DB below
  await mpage.screenshot({ path: "approved.png" });

  // ── criterion 4: the rejection path back ──
  console.log("\n[rejection path] clerk submits a 2nd invoice, manager rejects");
  page = await clerk.newPage();
  await page.goto(BASE + "/form/sales_invoice");
  await page.fill('input[name="customer"]', "Beta LLC");
  await page.fill('input[name="total"]', "0");
  await Promise.all([page.waitForLoadState("networkidle"), page.click('#doc-form button[type="submit"]')]);
  const key2 = page.url().split("/").pop();
  await page.click("text=+ Add row");
  await page.waitForSelector('input[name="lines.0.qty"]', { state: "visible" });
  await page.fill('input[name="lines.0.item"]', "Gadget");
  await page.fill('input[name="lines.0.qty"]', "3");
  await page.fill('input[name="lines.0.rate"]', "5.00");
  await page.fill('input[name="total"]', "15.00");
  await Promise.all([page.waitForLoadState("networkidle"), page.click('button[value="save"]')]);
  await Promise.all([page.waitForLoadState("networkidle"),
    page.click('form[action*="/transition/"] button[value="Submit"]')]);
  mpage = await mgr.newPage();
  await mpage.goto(`${BASE}/doc/sales_invoice/${key2}`);
  await Promise.all([mpage.waitForLoadState("networkidle"),
    mpage.click('form[action*="/transition/"] button[value="Reject"]')]);
  check((await badge(mpage)) === "Rejected", `2nd invoice "Rejected" (got "${await badge(mpage)}")`);
  // and the clerk can reopen it back to Draft
  page = await clerk.newPage();
  await page.goto(`${BASE}/doc/sales_invoice/${key2}`);
  btns = await actionButtons(page);
  check(btns.includes("Reopen"), `clerk sees "Reopen" on a Rejected doc (got [${btns}])`);
  await Promise.all([page.waitForLoadState("networkidle"),
    page.click('form[action*="/transition/"] button[value="Reopen"]')]);
  check((await badge(page)) === "Draft", `reopened back to "Draft" (got "${await badge(page)}")`);

  // expose the approved key for the DB docstatus check
  console.log(`\nAPPROVED_KEY=${key}`);
} finally {
  await browser.close();
}
console.log(`\n=== ${failures === 0 ? "ALL CHECKS PASSED" : failures + " CHECK(S) FAILED"} ===`);
process.exit(failures === 0 ? 0 : 1);
