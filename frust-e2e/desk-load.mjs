// WO-035 (gate assumption A3): Desk-tier CONCURRENT page-request throughput.
//
// WO-032 measured SSE subscribers but timed ordinary requests SEQUENTIALLY
// (`await` in a `for`), so Desk concurrency was never measured. Each Desk page
// handler makes a BLOCKING `ureq` call to the kernel inside an `async fn`,
// pinning a tokio worker for the whole round trip — so the cap is structurally
// near core-count. This measures it. It does not derive it: the arithmetic
// (16 workers / ~25 ms) is exactly what the v2.0 gate refuses.
//
//   node desk-load.mjs                 — the concurrency sweep + contention run
//
// Requires: surreal :8899 (scratch store), kernel :8790, desk :3000, seeded.
import http from "node:http";

const HOST = "127.0.0.1", PORT = 3000;
const PATH = "/list/widget";
const MODE = process.argv[2] || "all";   // "all" | "sweep" | "contention"
const RUNGS = [1, 10, 50, 200, 500];
const PER_RUNG_MS = 8000;

// keep-alive is mandatory: without it 500 concurrent clients exhaust Windows
// ephemeral ports and we'd measure the LOAD GENERATOR, not the Desk (the
// WO-032 near-miss, avoided by construction here).
const agent = new http.Agent({ keepAlive: true, maxSockets: Infinity, maxFreeSockets: 1024 });

function req(path, { method = "GET", headers = {}, body = null, timeout = 30000 } = {}) {
  return new Promise((resolve) => {
    const r = http.request({ host: HOST, port: PORT, path, method, headers, agent }, (res) => {
      let n = 0;
      res.on("data", (c) => (n += c.length));
      res.on("end", () => resolve({ status: res.statusCode, bytes: n, headers: res.headers }));
    });
    r.setTimeout(timeout, () => { r.destroy(); resolve({ status: 0, timedOut: true }); });
    r.on("error", () => resolve({ status: 0, error: true }));
    if (body) r.write(body);
    r.end();
  });
}

async function login() {
  const body = "user=mgr&pass=pw-mgr";
  const res = await req("/login-submit", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded", "Content-Length": Buffer.byteLength(body) },
    body,
  });
  const c = (res.headers["set-cookie"] || []).map((x) => x.split(";")[0]).join("; ");
  if (!c.includes("frust_session")) throw new Error("login failed " + res.status);
  return c;
}

const pct = (a, p) => (a.length ? a[Math.min(a.length - 1, Math.floor(a.length * p))] : 0);

// One rung: `conc` workers looping closed-loop for `ms`. Closed-loop (each
// client issues the next request only after its previous one returns) models
// real users and makes "concurrency" mean concurrent IN-FLIGHT requests.
async function rung(cookies, conc, ms, extraLabel = "") {
  const lat = [];
  let ok = 0, bad = 0, timeouts = 0;
  const codes = {};
  const stop = Date.now() + ms;
  const worker = async () => {
    while (Date.now() < stop) {
      const t = process.hrtime.bigint();
      const r = await req(PATH, { headers: { Cookie: cookies } });
      const dt = Number(process.hrtime.bigint() - t) / 1e6;
      lat.push(dt);
      if (r.timedOut) timeouts++;
      else if (r.status === 200) ok++;
      else { bad++; codes[r.status] = (codes[r.status] || 0) + 1; }
    }
  };
  const t0 = Date.now();
  await Promise.all(Array.from({ length: conc }, worker));
  const elapsed = (Date.now() - t0) / 1000;
  lat.sort((a, b) => a - b);
  return {
    conc, extraLabel, rps: ok / elapsed, ok, bad, timeouts, codes,
    p50: pct(lat, 0.5), p95: pct(lat, 0.95), p99: pct(lat, 0.99),
  };
}

function row(r) {
  const lbl = String(r.conc).padStart(4) + (r.extraLabel ? ` ${r.extraLabel}` : "");
  return `  ${lbl.padEnd(22)} ${r.rps.toFixed(1).padStart(7)} req/s   p50 ${r.p50.toFixed(0).padStart(5)}  p95 ${r.p95.toFixed(0).padStart(6)}  p99 ${r.p99.toFixed(0).padStart(6)} ms   ok ${r.ok}  bad ${r.bad}  timeouts ${r.timeouts}${Object.keys(r.codes).length ? '  codes ' + JSON.stringify(r.codes) : ''}`;
}

// hold an SSE stream open (for the contention phase)
function openSse(cookies, table, state) {
  return new Promise((resolve) => {
    const r = http.request(
      { host: HOST, port: PORT, path: `/live/sse/${table}`, method: "GET", headers: { Cookie: cookies, Accept: "text/event-stream" }, agent },
      (res) => {
        if (res.statusCode !== 200) { state.refused++; res.resume(); return resolve(null); }
        state.open++;
        res.on("data", (c) => { if (/^event:/m.test(c.toString())) state.events++; });
        res.on("error", () => {});
        resolve(res);
      }
    );
    r.on("error", () => { state.connectErrors++; resolve(null); });
    setTimeout(() => resolve(null), 15000);
    r.end();
    state.sockets.push(r);
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const cookies = await login();

console.log(`Desk concurrent page load — ${PATH}, closed-loop, ${PER_RUNG_MS / 1000}s per rung`);
console.log(`(16 cores => 16 tokio workers; each page handler blocks one for a kernel round trip)`);
const sweep = [];
if (MODE !== "contention") {
  console.log(`\nPHASE 1 — concurrency sweep, no SSE`);
  for (const c of RUNGS) {
    const r = await rung(cookies, c, PER_RUNG_MS);
    sweep.push(r);
    console.log(row(r));
    await sleep(500);
  }
  const best = sweep.reduce((a, b) => (b.rps > a.rps ? b : a));
  console.log(`\n  peak: ${best.rps.toFixed(1)} req/s at ${best.conc} concurrent`);
  if (MODE === "sweep") process.exit(0);
}

// The sweep ends with 500 clients hammering; opening SSE into a still-saturated
// stack is what made the first run's phase 2 report a void verdict.
console.log(`\n  (cooling down 20 s so the sweep cannot confound phase 2)`);
await sleep(20000);
console.log(`\nPHASE 2 — CONTENTION: page requests WHILE SSE streams hold the same 16-worker pool`);
const SSE_TABLES = ["widget", "widget2", "widget3", "widget4"];
const M = 48; // 12 per table — the kernel budget is 20/TABLE (WO-012)
const state = { open: 0, refused: 0, events: 0, connectErrors: 0, sockets: [] };
for (let i = 0; i < M; i++) { openSse(cookies, SSE_TABLES[i % SSE_TABLES.length], state); await sleep(250); }
await sleep(2500);
console.log(`  ${state.open} SSE streams live (refused ${state.refused}, connect errors ${state.connectErrors}), ${state.events} events so far`);

if (state.open < M * 0.8) {
  console.log(`  !! only ${state.open}/${M} streams live — contention phase would measure nothing; ABORTING phase 2 rather than reporting a void verdict`);
  for (const s2 of state.sockets) s2.destroy();
  process.exit(2);
}
if (MODE === "contention") {
  console.log(`  taking baselines WITH the streams already live`);
}
const contend = [];
for (const c of [50, 200]) {
  const before = state.events;
  const r = await rung(cookies, c, PER_RUNG_MS, `+${state.open}sse`);
  r.sseEvents = state.events - before;
  contend.push(r);
  console.log(row(r) + `   SSE events during: ${r.sseEvents}`);
  await sleep(500);
}

console.log(`\nVERDICT (A3)`);
for (const c of contend) {
  const baseline = sweep.find((s) => s.conc === c.conc);
  if (baseline) console.log(`  ${c.conc} concurrent: ${baseline.rps.toFixed(1)} -> ${c.rps.toFixed(1)} req/s with ${state.open} SSE (${((c.rps / baseline.rps) * 100).toFixed(0)}% of baseline)`);
  else console.log(`  ${c.conc} concurrent WITH ${state.open} SSE live: ${c.rps.toFixed(1)} req/s  p50 ${c.p50.toFixed(0)} ms  bad ${c.bad}`);
  // the interesting half: did the SSE side keep ticking while pages hammered?
  const expected = (state.open * PER_RUNG_MS) / 1000; // ~1 event/stream/sec
  console.log(`     SSE kept ticking: ${c.sseEvents} events (rough expectation ~${expected.toFixed(0)}) — ${c.sseEvents > expected * 0.5 ? "NOT starved" : "STARVED"}`);
}
for (const s of state.sockets) s.destroy();
process.exit(0);
