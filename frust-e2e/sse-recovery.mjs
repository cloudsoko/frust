// WO-038 criterion 2: the POST-BURST realtime recovery window.
//
// WO-035 measured the impairment directly: after a load burst, **47 of 48 SSE
// subscribes were refused at +20 s**, and it only recovered by ~60 s. A spike
// locking realtime out for a minute is the reconnect-storm failure this
// criterion exists to bound.
//
// The contention run in `desk-load.mjs` establishes streams BEFORE the load,
// which is a different question — it shows live streams are not starved. This
// one reproduces WO-035's scenario exactly: burst first, then try to subscribe
// at intervals, and report the window.
//
//   node sse-recovery.mjs
//
// Requires the same stack as desk-load.mjs.
import http from "node:http";

const HOST = "127.0.0.1", PORT = 3000;
const BURST_CONC = 200, BURST_MS = 8000;
const PROBES = 48;
const AT_SECONDS = [0, 5, 20, 40, 60];

const agent = new http.Agent({ keepAlive: true, maxSockets: Infinity, maxFreeSockets: 1024 });

function req(path, { method = "GET", headers = {}, body = null, timeout = 15000 } = {}) {
  return new Promise((resolve) => {
    const r = http.request({ host: HOST, port: PORT, path, method, headers, agent }, (res) => {
      let n = 0;
      res.on("data", (c) => (n += c.length));
      res.on("end", () => resolve({ status: res.statusCode, bytes: n }));
    });
    r.setTimeout(timeout, () => { r.destroy(); resolve({ status: 0, timedOut: true }); });
    r.on("error", () => resolve({ status: 0, error: true }));
    if (body) r.write(body);
    r.end();
  });
}

async function login() {
  const body = "user=mgr&pass=pw-mgr";
  const res = await new Promise((resolve) => {
    const r = http.request(
      { host: HOST, port: PORT, path: "/login-submit", method: "POST", agent,
        headers: { "Content-Type": "application/x-www-form-urlencoded", "Content-Length": Buffer.byteLength(body) } },
      (res) => { res.resume(); res.on("end", () => resolve(res)); },
    );
    r.on("error", () => resolve({ headers: {} }));
    r.write(body); r.end();
  });
  const c = (res.headers["set-cookie"] || []).map((x) => x.split(";")[0]).join("; ");
  if (!c.includes("frust_session")) throw new Error("login failed");
  return c;
}

/// Open an SSE stream and report whether the SUBSCRIBE was accepted. The
/// stream is torn down immediately — this measures admission to realtime, not
/// stream longevity.
function probeSubscribe(cookies, table) {
  return new Promise((resolve) => {
    const r = http.request(
      { host: HOST, port: PORT, path: `/live/sse/${table}`, method: "GET", agent,
        headers: { Cookie: cookies, Accept: "text/event-stream" } },
      (res) => {
        // 200 = the Desk accepted and the kernel subscribed; anything else is
        // a refusal, and 503 specifically is an honest shed
        const status = res.statusCode;
        res.destroy();
        resolve(status);
      },
    );
    r.setTimeout(10000, () => { r.destroy(); resolve(0); });
    r.on("error", () => resolve(0));
    r.end();
  });
}

async function burst(cookies) {
  const deadline = Date.now() + BURST_MS;
  const worker = async () => {
    while (Date.now() < deadline) {
      await req("/list/widget", { headers: { Cookie: cookies } });
    }
  };
  await Promise.all(Array.from({ length: BURST_CONC }, worker));
}

const cookies = await login();

// Baseline: unloaded, so "refused" has something to be compared against.
const base = await Promise.all(
  Array.from({ length: PROBES }, (_, i) => probeSubscribe(cookies, `widget${i % 4 === 0 ? "" : i % 4 + 1}`)),
);
const baseOk = base.filter((s) => s === 200).length;
console.log(`baseline (unloaded): ${baseOk}/${PROBES} subscribes accepted`);
// The baseline is the instrument's own control. A run where NOTHING can
// subscribe even unloaded would report "recovered by +0s" on 0 >= 0 — a
// vacuous pass. Refuse to produce a verdict rather than a false one.
if (baseOk === 0) {
  console.log(`
ABORT: 0/${PROBES} subscribes work even UNLOADED (saw ${JSON.stringify(base.slice(0, 5))}).`);
  console.log("The probe is broken, not the Desk. No verdict.");
  process.exit(2);
}

console.log(`\ndriving ${BURST_CONC} concurrent for ${BURST_MS / 1000}s ...`);
await burst(cookies);
const burstEnd = Date.now();

console.log("\npost-burst subscribe probes (WO-035 measured 47/48 REFUSED at +20s):");
let recovered = null;
for (const at of AT_SECONDS) {
  const wait = burstEnd + at * 1000 - Date.now();
  if (wait > 0) await new Promise((r) => setTimeout(r, wait));
  const res = await Promise.all(
    Array.from({ length: PROBES }, (_, i) => probeSubscribe(cookies, `widget${i % 4 === 0 ? "" : i % 4 + 1}`)),
  );
  const ok = res.filter((s) => s === 200).length;
  const shed = res.filter((s) => s === 503).length;
  const other = res.filter((s) => s !== 200 && s !== 503);
  console.log(
    `  +${String(at).padStart(2)}s : ${String(ok).padStart(2)}/${PROBES} accepted` +
    `   503 (honest shed) ${String(shed).padStart(2)}` +
    (other.length ? `   OTHER ${JSON.stringify(other.slice(0, 5))}` : "   other 0"),
  );
  if (recovered === null && ok >= baseOk) recovered = at;
}

console.log(
  `\nRECOVERY WINDOW: ${recovered === null ? "NOT recovered within " + AT_SECONDS.at(-1) + "s" : "recovered by +" + recovered + "s"}` +
  ` (baseline ${baseOk}/${PROBES})`,
);
process.exit(recovered === null ? 1 : 0);
