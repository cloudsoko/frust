// WO-032 criteria 1, 3, 4 — the committed, re-runnable browser proof.
// (Methodology ruling 2026-07-28: MCP may drive, but the seal is a committed
// spec. An observation you cannot re-run is how an overclaim survives.)
//
//   node sse.spec.mjs        — requires kernel :8790, desk :3000, surreal :8899
import { chromium } from "playwright";

const BASE = "http://127.0.0.1:3000";
const KERNEL = "http://127.0.0.1:8790";
let failures = 0;
const check = (cond, msg) => { console.log((cond ? "  PASS " : "  FAIL ") + msg); if (!cond) failures++; };

async function login(ctx, user, pass) {
  const page = await ctx.newPage();
  await page.goto(BASE + "/login");
  await page.fill('input[name="user"]', user);
  await page.fill('input[name="pass"]', pass);
  await Promise.all([page.waitForLoadState("networkidle"), page.click('button[type="submit"], input[type="submit"]')]);
  return page;
}

// an out-of-band write, straight to the kernel — nothing to do with the browser
async function kernelWrite(user, pass, doc) {
  const login = await fetch(`${KERNEL}/login`, { method: "POST", body: JSON.stringify({ user, pass }) });
  const { token } = await login.json();
  const res = await fetch(`${KERNEL}/write/sales_invoice`, {
    method: "POST",
    headers: { Authorization: `Bearer ${token}` },
    body: JSON.stringify({ doc }),
  });
  return res.status;
}

const browser = await chromium.launch();
try {
  // ── criterion 1: SSE replaces polling ──
  console.log("\n[criterion 1] one long-lived SSE stream, zero browser polls");
  const clerk = await browser.newContext();
  let page = await login(clerk, "clerk1", "pw-clerk1");
  const seen = { sse: 0, polls: 0 };
  page.on("request", (r) => {
    const u = r.url();
    if (u.includes("/live/sse/")) seen.sse++;
    if (u.includes("/live/events/")) seen.polls++;
  });
  await page.goto(BASE + "/list/sales_invoice");
  await page.waitForTimeout(6000); // >> the old 3 s poll interval

  check(seen.sse === 1, `exactly one SSE connection opened (got ${seen.sse})`);
  check(seen.polls === 0, `zero /live/events polls in 6 s — polling retired (got ${seen.polls})`);

  // ── criterion 1b: self-refresh on an out-of-band write ──
  console.log("\n[criterion 1b] an out-of-band write pushes a refresh");
  let reloaded = false;
  page.on("framenavigated", (f) => { if (f === page.mainFrame()) reloaded = true; });
  const status = await kernelWrite("clerk1", "pw-clerk1", {
    customer: { kind: "text", v: "SSE Push Test" },
    total: { kind: "decimal", v: "0" },
  });
  check(status === 200, `out-of-band kernel write accepted (status ${status})`);
  await page.waitForTimeout(5000);
  check(reloaded, "the watching page refreshed itself from the SSE tick (no user action)");

  // ── criterion 3: permission-aware push ──
  // The kernel's LIVE subscription runs under the SUBSCRIBER's own JWT
  // (WO-011), so a clerk must not be woken by a row they cannot read. A
  // manager-owned invoice is invisible to clerk1 under ADR-012.
  console.log("\n[criterion 3] a clerk is not woken by rows they cannot see");
  const clerk2 = await browser.newContext();
  const page2 = await login(clerk2, "clerk1", "pw-clerk1");
  await page2.goto(BASE + "/list/sales_invoice");
  await page2.waitForTimeout(2500);
  let woken = false;
  page2.on("framenavigated", (f) => { if (f === page2.mainFrame()) woken = true; });
  const mgrStatus = await kernelWrite("manager", "pw-manager", {
    customer: { kind: "text", v: "Manager Private" },
    total: { kind: "decimal", v: "0" },
  });
  check(mgrStatus === 200, `manager's out-of-band write accepted (status ${mgrStatus})`);
  await page2.waitForTimeout(5000);
  check(!woken, "clerk's stream did NOT tick on a manager-owned row (zero-leak preserved on the SSE path)");

  // ── criterion 4: graceful degradation ──
  // Kill the SSE route at the network layer; the page must fall back to polling
  // rather than losing realtime entirely (REQ-6.5.2: an enhancement, never a
  // correctness dependency).
  console.log("\n[criterion 4] SSE failure falls back to polling");
  const fb = await browser.newContext();
  const page3 = await login(fb, "clerk1", "pw-clerk1");
  await page3.route("**/live/sse/**", (route) => route.abort());
  const fbSeen = { polls: 0 };
  page3.on("request", (r) => { if (r.url().includes("/live/events/")) fbSeen.polls++; });
  await page3.goto(BASE + "/list/sales_invoice");
  await page3.waitForTimeout(8000);
  check(fbSeen.polls > 0, `with SSE dead the page polls instead (got ${fbSeen.polls} polls) — realtime degraded, not lost`);
} finally {
  await browser.close();
}
console.log(`\n=== ${failures === 0 ? "ALL CHECKS PASSED" : failures + " CHECK(S) FAILED"} ===`);
process.exit(failures === 0 ? 0 : 1);
