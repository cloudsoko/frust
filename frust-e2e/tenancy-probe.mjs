// WO-039: the multi-DB per-tenant tenancy PROBE (Milestone 4 opener).
//
// ADR-003 always envisioned database-per-tenant; v0 shipped single-DB, and
// WO-027 measured the cost: per-database export means restore-one = restore-all.
// This probe answers whether per-tenant DATABASES are clean, before the build.
//
// Note the kernel already models it: `Db::tenant()` returns `cfg.db` — a tenant
// IS a database. What is unbuilt is one PROCESS routing to N tenant databases.
//
//   node tenancy-probe.mjs
//
// Requires: surreal on :8899 (use a SCRATCH data-dir), and surreal.exe on PATH
// or at ../frust-skel/surreal.exe for the export/import leg.
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const EP = "http://127.0.0.1:8899";
const NS = "frust";
const SURREAL = "../frust-skel/surreal.exe";
let fail = 0;
const check = (c, m) => { console.log((c ? "  PASS " : "  FAIL ") + m); if (!c) fail++; };

async function sql(db, q, auth = { user: "root", pass: "root" }) {
  const headers = { Accept: "application/json", "surreal-ns": NS };
  if (db) headers["surreal-db"] = db;
  if (auth.token) headers.Authorization = `Bearer ${auth.token}`;
  else headers.Authorization = "Basic " + Buffer.from(`${auth.user}:${auth.pass}`).toString("base64");
  const r = await fetch(`${EP}/sql`, { method: "POST", headers, body: q });
  return { status: r.status, body: await r.json().catch(() => null) };
}

// Each tenant gets its OWN database, with its own DEFINE ACCESS (its own signing
// key) and its own data. That last part is what makes isolation DB-enforced.
async function makeTenant(db, who, secret) {
  await sql(null, `DEFINE DATABASE IF NOT EXISTS ${db};`);
  await sql(db, `
    DEFINE TABLE OVERWRITE app_user SCHEMAFULL
      PERMISSIONS FOR select WHERE id = $auth.id FOR create, update, delete NONE;
    DEFINE FIELD OVERWRITE name ON app_user TYPE string;
    DEFINE FIELD OVERWRITE role ON app_user TYPE string;
    DEFINE FIELD OVERWRITE pass ON app_user TYPE string PERMISSIONS NONE;
    DEFINE ACCESS OVERWRITE account ON DATABASE TYPE RECORD
      SIGNIN (SELECT * FROM app_user WHERE name = $name AND crypto::argon2::compare(pass, $pass))
      WITH JWT ALGORITHM HS512 KEY '${secret}'
      DURATION FOR TOKEN 1h, FOR SESSION 12h;
    DEFINE TABLE OVERWRITE secret_doc SCHEMALESS PERMISSIONS FULL;
    CREATE app_user:u SET name = '${who}', role = 'manager', pass = crypto::argon2::generate('pw');
    CREATE secret_doc SET owner_tenant = '${db}', body = 'confidential-${db}';
  `);
}

async function signin(db) {
  const r = await fetch(`${EP}/signin`, {
    method: "POST",
    headers: { Accept: "application/json", "Content-Type": "application/json" },
    body: JSON.stringify({ ns: NS, db, ac: "account", name: `user-${db}`, pass: "pw" }),
  });
  const j = await r.json().catch(() => null);
  return j?.token ?? null;
}

const A = "tenant_a", B = "tenant_b", A_RESTORED = "tenant_a_restored";
for (const d of [A, B, A_RESTORED]) await sql(null, `REMOVE DATABASE IF EXISTS ${d};`);
// DIFFERENT signing keys per tenant — the tenancy boundary in one line
await makeTenant(A, `user-${A}`, "key-for-tenant-a-0000000000000000");
await makeTenant(B, `user-${B}`, "key-for-tenant-b-1111111111111111");

console.log("\n[1] ISOLATION — is it DB-enforced, or app-layer?");
const tokA = await signin(A);
check(!!tokA, "tenant A's user can sign in to tenant A");

// A's own database: readable
const ownRead = await sql(A, "SELECT body FROM secret_doc;", { token: tokA });
const ownRows = ownRead.body?.[0]?.result ?? [];
check(ownRows.length === 1 && ownRows[0].body === `confidential-${A}`, "A reads A's data");

// THE TEST: A's token presented WITH A HEADER NAMING B's DATABASE.
//
// The assertion that matters is *whose data comes back* — not the status code.
// (First version of this probe asserted "non-200 or zero rows" and reported a
// false leak: the call returns 200 with ONE row, and the row is A's own.)
const cross = await sql(B, "SELECT body, owner_tenant FROM secret_doc;", { token: tokA });
const crossRows = cross.body?.[0]?.result ?? [];
const leaked = crossRows.filter((r) => r.owner_tenant === B);
check(leaked.length === 0, `NO tenant-B row is reachable with A's token (leaked ${leaked.length})`);
check(
  crossRows.every((r) => r.owner_tenant === A),
  "the session stayed pinned to A — A's credential can only ever see A's data"
);
// Why: the JWT carries its own ns/db claims (ns=frust, db=tenant_a) and
// SurrealDB binds the session to THOSE, ignoring a conflicting surreal-db
// header. A caller cannot address another tenant's database at all. This is
// DB-enforced by the credential itself — no kernel permission clause is
// involved, so no kernel bug can widen it.
const garbage = await sql(B, "SELECT 1;", { token: "not.a.real.token" });
check(garbage.status === 401, `a forged token is refused (401), so tokens are genuinely validated (got ${garbage.status})`);

console.log("\n[2] PER-TENANT RESTORE — the P-8.1 unlock");
const tmp = mkdtempSync(join(tmpdir(), "wo039-"));
const dump = join(tmp, "tenant_a.surql");
try {
  execFileSync(SURREAL, ["export", "--endpoint", EP, "-u", "root", "-p", "root", "--ns", NS, "--db", A, dump], { stdio: "pipe" });
  console.log("  exported tenant A alone");
  // mutate B AFTER the export, so "B untouched" is a claim with teeth
  await sql(B, "CREATE secret_doc SET owner_tenant = 'tenant_b', body = 'written-after-export';");
  await sql(null, `DEFINE DATABASE ${A_RESTORED};`);
  execFileSync(SURREAL, ["import", "--endpoint", EP, "-u", "root", "-p", "root", "--ns", NS, "--db", A_RESTORED, dump], { stdio: "pipe" });

  const ra = (await sql(A_RESTORED, "SELECT body FROM secret_doc;")).body?.[0]?.result ?? [];
  check(ra.length === 1 && ra[0].body === `confidential-${A}`, "tenant A restored into a FRESH database, intact");

  const rb = (await sql(B, "SELECT body FROM secret_doc ORDER BY body;")).body?.[0]?.result ?? [];
  check(rb.length === 2, `tenant B UNTOUCHED by A's restore (${rb.length} rows, incl. the post-export write)`);
  check(rb.some((r) => r.body === "written-after-export"), "B's post-export write survived — the restore did not roll B back");
} finally {
  rmSync(tmp, { recursive: true, force: true });
}

console.log(`\n=== ${fail === 0 ? "PROBE CLEAN" : fail + " CHECK(S) FAILED"} ===`);
process.exit(fail === 0 ? 0 : 1);
