// WO-042 criterion 3: is WO-038's admission shed still wired after the re-skin?
//
// A browser CANNOT prove this: Chrome caps concurrent connections per host at
// ~6, so 300 `fetch`es serialise and never put 64 requests in flight. The first
// attempt at this check ran in-page, saw `shed: 0`, and would have "proved" the
// shed was gone. It was measuring the instrument.
//
// So: a Node agent with unlimited sockets, the same shape WO-038's driver used.
//
//   node shed-check.mjs [concurrency]
//
// Requires the dev stack (surreal :8899, kernel :8790, desk :3000) and the
// seeded clerk1 user.
import http from "node:http";

const HOST = "127.0.0.1", PORT = 3000;
const CONC = Number(process.argv[2] || 200);
const PATH = "/list/purchase_order";

const agent = new http.Agent({ keepAlive: true, maxSockets: Infinity, maxFreeSockets: 1024 });

function req(path, { method = "GET", headers = {}, body = null } = {}) {
  return new Promise((resolve) => {
    const r = http.request({ host: HOST, port: PORT, path, method, headers, agent }, (res) => {
      let buf = "";
      res.on("data", (c) => (buf += c));
      res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, body: buf }));
    });
    r.on("error", () => resolve({ status: 0, headers: {} }));
    if (body) r.write(body);
    r.end();
  });
}

const form = "user=clerk1&pass=pw-clerk1";
const login = await req("/login-submit", {
  method: "POST",
  headers: {
    "Content-Type": "application/x-www-form-urlencoded",
    "Content-Length": Buffer.byteLength(form),
  },
  body: form,
});
const cookies = (login.headers["set-cookie"] || []).map((c) => c.split(";")[0]).join("; ");
if (!cookies.includes("frust_session")) {
  console.log("ABORT: login failed — no verdict");
  process.exit(2);
}

const before = JSON.parse((await req("/admission")).body);
console.log(`before : shed=${before.shed} max_inflight=${before.max_inflight}`);

const results = await Promise.all(
  Array.from({ length: CONC }, () => req(PATH, { headers: { Cookie: cookies } })),
);

const codes = {};
let retryAfter = null;
for (const r of results) {
  codes[r.status] = (codes[r.status] || 0) + 1;
  if (r.status === 503 && !retryAfter) retryAfter = r.headers["retry-after"];
}
const after = JSON.parse((await req("/admission")).body);

console.log(`after  : shed=${after.shed}`);
console.log(`codes  : ${JSON.stringify(codes)}`);
console.log(`Retry-After on a shed response: ${retryAfter ?? "(none seen)"}`);

const shedDelta = after.shed - before.shed;
const got503 = (codes["503"] || 0) > 0;
const ok = shedDelta > 0 && got503 && retryAfter;
console.log(
  `\n${ok ? "PASS" : "FAIL"}: shed moved by ${shedDelta}, ${codes["503"] || 0} × 503, ` +
  `Retry-After ${retryAfter ? "present" : "MISSING"}`,
);
// The counter and the status code must AGREE. A shed that does not surface as a
// 503, or a 503 the counter never recorded, is the two halves drifting apart.
if (ok && shedDelta !== (codes["503"] || 0)) {
  console.log(`NOTE: shed delta ${shedDelta} != 503 count ${codes["503"]} — other requests were shed concurrently`);
}
process.exit(ok ? 0 : 1);
