// WO-032 criterion 2: does an SSE subscriber cost an OS thread?
// Holds N >> core-count concurrent SSE streams open against the Desk while
// timing ordinary page requests, and watches the Desk's OS thread count.
//
// The hypothesis under test: if the SSE handler blocks a tokio worker for the
// subscription's lifetime, then at N >= worker-threads the Desk stops serving
// ordinary requests. Falsified if thread count stays flat and latency holds.
import http from "node:http";
import { execSync } from "node:child_process";

const HOST = "127.0.0.1", PORT = 3000;
const N = Number(process.argv[2] || 64);
// spread across tables: the kernel budget is 20 subs/TABLE (WO-012)
const TABLES = ["sales_invoice", "customer", "item", "payment", "invoice", "invoice_line", "purchase_order", "ar_outstanding"];

function req(path, { method = "GET", headers = {}, body = null, timeout = 10000 } = {}) {
  return new Promise((resolve, reject) => {
    const r = http.request({ host: HOST, port: PORT, path, method, headers }, (res) => {
      let data = "";
      res.on("data", (c) => (data += c));
      res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, body: data }));
    });
    // A request that never answers is the FAILURE this bench exists to detect
    // (all workers blocked), so it must time out and be reported, not hang.
    r.setTimeout(timeout, () => { r.destroy(); resolve({ status: 0, headers: {}, body: "", timedOut: true }); });
    r.on("error", () => resolve({ status: 0, headers: {}, body: "", timedOut: true }));
    if (body) r.write(body);
    r.end();
  });
}

async function login() {
  const body = "user=manager&pass=pw-manager";
  const res = await req("/login-submit", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded", "Content-Length": Buffer.byteLength(body) },
    body,
  });
  const cookies = (res.headers["set-cookie"] || []).map((c) => c.split(";")[0]).join("; ");
  if (!cookies.includes("frust_session")) throw new Error("login failed: " + res.status);
  return cookies;
}

function threadCount() {
  try {
    return Number(execSync(
      `powershell -NoProfile -Command "(Get-Process frust-desk -ErrorAction SilentlyContinue).Threads.Count"`,
      { encoding: "utf8" }
    ).trim().split(/\s+/)[0]);
  } catch { return -1; }
}

// Open one SSE stream; resolves once the first event arrives (proving it is live).
function openSse(cookies, table, state) {
  return new Promise((resolve, reject) => {
    const r = http.request(
      { host: HOST, port: PORT, path: `/live/sse/${table}`, method: "GET", headers: { Cookie: cookies, Accept: "text/event-stream" } },
      (res) => {
        if (res.statusCode !== 200) { state.refused++; res.resume(); return resolve(null); }
        state.open++;
        let first = false;
        res.on("data", (chunk) => {
          const s = chunk.toString();
          if (/^event:/m.test(s)) {
            state.events++;
            if (!first) { first = true; resolve(res); }
          }
        });
        res.on("error", () => {});
      }
    );
    // A connect error is the LOAD GENERATOR's limit (Windows ephemeral ports /
    // TIME_WAIT), not the Desk's — count it separately so the two can never be
    // confused in the verdict.
    r.on("error", (e) => { state.connectErrors++; resolve(null); });
    // A stream that never delivers a first event within 15 s is STALLED — the
    // naive-blocking failure mode also starves NEW connections, so without this
    // the bench hangs instead of reporting.
    setTimeout(() => { state.stalled++; resolve(null); }, 15000);
    r.end();
    state.sockets.push(r);
  });
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function latency(cookies, n = 20) {
  const ms = [];
  let timeouts = 0;
  for (let i = 0; i < n; i++) {
    const t = process.hrtime.bigint();
    const res = await req("/list/sales_invoice", { headers: { Cookie: cookies }, timeout: 10000 });
    ms.push(Number(process.hrtime.bigint() - t) / 1e6);
    if (res.timedOut) timeouts++;
  }
  ms.sort((a, b) => a - b);
  return {
    p50: ms[Math.floor(ms.length * 0.5)], p95: ms[Math.floor(ms.length * 0.95)],
    max: ms[ms.length - 1], timeouts,
  };
}

const cookies = await login();
console.log(`subscribers: ${N}   tables: ${TABLES.length}   (kernel budget: 20/table)`);

const idle = threadCount();
const base = await latency(cookies);
console.log(`\nBASELINE (0 subscribers)`);
console.log(`  desk OS threads : ${idle}`);
console.log(`  ordinary GET    : p50 ${base.p50.toFixed(1)} ms   p95 ${base.p95.toFixed(1)} ms   timeouts ${base.timeouts}`);

const state = { open: 0, refused: 0, events: 0, connectErrors: 0, stalled: 0, sockets: [] };
const streams = [];
for (let i = 0; i < N; i++) {
  streams.push(openSse(cookies, TABLES[i % TABLES.length], state));
  await sleep(25); // stagger: a tight open loop exhausts local ports, not the Desk
}
await Promise.all(streams);
// let every stream tick at least once more (drain interval is 1 s)
await new Promise((r) => setTimeout(r, 3000));

const loadedThreads = threadCount();
const loaded = await latency(cookies);
console.log(`\nUNDER ${state.open} LIVE SSE STREAMS (kernel-budget refusals: ${state.refused}, client connect errors: ${state.connectErrors}, STALLED-never-ticked: ${state.stalled})`);
console.log(`  desk OS threads : ${loadedThreads}  (delta ${loadedThreads - idle})`);
console.log(`  events received : ${state.events}`);
console.log(`  ordinary GET    : p50 ${loaded.p50.toFixed(1)} ms   p95 ${loaded.p95.toFixed(1)} ms   timeouts ${loaded.timeouts}`);

const perSub = (loadedThreads - idle) / Math.max(state.open, 1);
const healthy = loaded.timeouts === 0 && state.stalled === 0 && loaded.p50 < 2000;
console.log(`\nVERDICT`);
// THE DISCRIMINATING MEASURES. Worker starvation is the real failure mode, and
// it shows up here — not in the thread count.
console.log(`  streams live / stalled    : ${state.open} / ${state.stalled}`);
console.log(`  ordinary-request p50 ratio: ${(loaded.p50 / base.p50).toFixed(2)}x   timeouts ${loaded.timeouts}/20`);
console.log(`  DESK STILL SERVING        : ${healthy ? "YES" : "NO — STALLED"}`);
// NOT a discriminator, kept only to record it and to say why (WO-032 finding):
// tokio never grows its worker pool for BLOCKED tasks, so a fully-starved Desk
// reports the same 0.000 as a healthy one. Measured at 0.000 in both the
// correct build (160 subs, healthy) and the naive control (24 subs, stalled).
console.log(`  [non-discriminating] OS threads/subscriber: ${perSub.toFixed(3)} — reads 0.000 even when STALLED; starvation is not thread growth`);
process.exitCode = healthy ? 0 : 1;

for (const s of state.sockets) s.destroy();
process.exit(process.exitCode ?? 0);
