import { closeSync, mkdirSync, openSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn, spawnSync } from "node:child_process";
import { setupFixture } from "./fixture.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const MCP_ROOT = resolve(HERE, "..");
const REPO = resolve(MCP_ROOT, "..");
const KERNEL_ROOT = join(REPO, "frust-kernel");
const RUNTIME = join(MCP_ROOT, "scratch", "runtime");
const SURREAL = "D:\\Dev\\rust\\frust-bench\\surreal.exe";
const KERNEL_EXE = join(KERNEL_ROOT, "target", "release", "frust.exe");
const DB_BASE = "http://127.0.0.1:8890";
const KERNEL_BASE = "http://127.0.0.1:8795";
const MCP_URL = "http://127.0.0.1:8796/mcp";
const children = [];
const descriptors = [];

function command(command, args, options = {}) {
  const result = spawnSync(command, args, { cwd: MCP_ROOT, stdio: "inherit", windowsHide: true, ...options });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} exited ${result.status}`);
}

async function waitHttp(url, timeout = 20000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try { if ((await fetch(url)).ok) return; } catch {}
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  throw new Error(`timed out waiting for ${url}`);
}

async function requireFree(url, label) {
  try {
    await fetch(url);
    throw new Error(`${label} is already listening; full proof requires its isolated port`);
  } catch (error) {
    if (error.message.includes("already listening")) throw error;
  }
}

function logDescriptor(name) {
  const fd = openSync(join(RUNTIME, name), "w");
  descriptors.push(fd);
  return fd;
}

function start(commandName, args, options) {
  const child = spawn(commandName, args, { windowsHide: true, ...options });
  children.push(child);
  child.on("exit", (code) => {
    if (code && code !== 0) console.error(`${commandName} exited ${code}`);
  });
  return child;
}

function seedIdentities() {
  const input = readFileSync(join(MCP_ROOT, "scratch", "seed.surql"), "utf8");
  const result = spawnSync(SURREAL, [
    "sql", "--endpoint", DB_BASE, "--username", "root", "--password", "root",
    "--auth-level", "root", "--multi",
  ], { input, encoding: "utf8", windowsHide: true, cwd: RUNTIME });
  if (result.status !== 0) throw new Error(`identity seed failed: ${result.stderr || result.stdout}`);
  if (/\b(error|failed)\b/i.test(result.stdout ?? "")) throw new Error(`identity seed reported an error: ${result.stdout}`);
}

function runProof(file, extraEnv = {}) {
  command(process.execPath, [join(HERE, file)], {
    env: { ...process.env, FRUST_BASE: KERNEL_BASE, FRUST_MCP_URL: MCP_URL, ...extraEnv },
  });
}

async function main() {
  mkdirSync(RUNTIME, { recursive: true });
  mkdirSync(join(RUNTIME, "mail"), { recursive: true });
  await requireFree(`${DB_BASE}/health`, "SurrealDB port 8890");
  await requireFree(`${KERNEL_BASE}/ready`, "kernel port 8795");
  await requireFree("http://127.0.0.1:8796/health", "MCP port 8796");

  if (!/^(1|true|yes)$/i.test(process.env.FRUST_SKIP_BUILD ?? "")) {
    command("cargo", ["build", "--release", "-p", "frust-kernel"], { cwd: KERNEL_ROOT });
  }

  const surrealLog = logDescriptor("surreal.log");
  start(SURREAL, ["start", "--user", "root", "--pass", "root", "--bind", "127.0.0.1:8890", "memory"], {
    cwd: RUNTIME, stdio: ["ignore", surrealLog, surrealLog],
  });
  await waitHttp(`${DB_BASE}/health`);
  // /health can answer just before root SQL is ready on a new in-memory store.
  await new Promise((resolve) => setTimeout(resolve, 1000));
  seedIdentities();

  const kernelLogPath = join(RUNTIME, "kernel.log");
  const kernelLog = logDescriptor("kernel.log");
  start(KERNEL_EXE, ["serve"], {
    cwd: MCP_ROOT,
    stdio: ["ignore", kernelLog, kernelLog],
    env: {
      ...process.env,
      FRUST_DB_ENDPOINT: DB_BASE,
      FRUST_ADDR: "127.0.0.1:8795",
      FRUST_TENANT: "frustmcp",
      FRUST_TENANCY: "database-per-tenant",
      FRUST_ARTIFACTS: join(REPO, "wasm-spike", "artifacts-old-world"),
      FRUST_MAIL: "file",
      FRUST_MAIL_DIR: join(RUNTIME, "mail"),
      FRUST_LOG: "info",
    },
  });
  await waitHttp(`${KERNEL_BASE}/ready`, 30000);
  process.env.FRUST_BASE = KERNEL_BASE;
  await setupFixture();

  const mcpLog = logDescriptor("mcp.log");
  start(process.execPath, [join(MCP_ROOT, "src", "server.mjs")], {
    cwd: MCP_ROOT,
    stdio: ["ignore", mcpLog, mcpLog],
    env: {
      ...process.env,
      FRUST_BASE: KERNEL_BASE,
      FRUST_MCP_HOST: "127.0.0.1",
      FRUST_MCP_PORT: "8796",
      FRUST_MCP_POLL_MS: "200",
      FRUST_MCP_WRITE_EXPOSURE: JSON.stringify({
        expense_claim: ["create", "update", "submit", "delete"],
        mcp_activity: ["create"],
      }),
    },
  });
  await waitHttp("http://127.0.0.1:8796/health");

  runProof("unit.mjs");
  runProof("fidelity.mjs", { FRUST_KERNEL_LOG: kernelLogPath });
  runProof("containment.mjs");
  runProof("subscriptions.mjs");
  console.log("\nPASS  full: all delete-exposure proofs passed on isolated SurrealDB port 8890");
}

async function cleanup() {
  for (const child of children.reverse()) {
    if (!child.killed) child.kill();
  }
  await new Promise((resolve) => setTimeout(resolve, 250));
  for (const fd of descriptors) {
    try { closeSync(fd); } catch {}
  }
}

main().catch((error) => {
  console.error(`\nFULL PROOF FAILED: ${error.stack ?? error}`);
  process.exitCode = 1;
}).finally(cleanup);
