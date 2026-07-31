# frust-e2e — the evidence harnesses

The browser proofs and load benches behind several work-order closures and three
v2.0-gate assumptions. These drive the **running stack** (surreal + kernel +
Desk); they are not `cargo test` unit tests and are not run by the suite.

They live here because the project's methodology rests on evidence being
**re-runnable**. Evidence you cannot re-run is an anecdote — and these were
previously a loose `wf-proof/` directory on an ad-hoc install, which is how
that happens.

## Prerequisites

Three processes, in this order. All commands from `D:\Dev\rust`.

```bash
# 1. the database (dev store; use a scratch dir for perf runs — see below)
cd frust-skel && ./surreal.exe start --user root --pass root \
    --bind 127.0.0.1:8899 surrealkv://D:/Dev/rust/frust-skel/data

# 2. the kernel  (boots ns `frust`, db `skeleton`; :8790)
cd frust-kernel && ./target/release/frust.exe serve

# 3. the Desk    (:3000)
cd frust-desk && ./target/debug/frust-desk.exe
```

Then, once: `cd frust-e2e && pnpm install` (pnpm, not npm — npm project
installs fail machine-wide on this box).

Seed data expectations: a `manager`/`pw-manager` and `clerk1`/`pw-clerk1` user,
and the doctype each harness names. The load driver needs its own seeded
doctypes (see its header).

## The harnesses

| script | proves | WO |
|---|---|---|
| `pnpm workflow` | The seed approval flow **clicked end to end** in real Chromium: clerk creates → Submit (docstatus 0→0) → manager Approve (0→1) → Reject → Reopen, plus role-filtered buttons, `ROLE_DENIED` surfaced as prose, dirty-guard intact. 18 checks. | WO-031 |
| `pnpm sse` | SSE replaced polling: **1 stream, 0 polls**; out-of-band write self-refreshes the page; a clerk is **not** woken by a row it cannot read; SSE failure falls back to polling. 8 checks. | WO-032 |
| `pnpm sse-bench` | SSE subscribers cost **no pinned OS thread**: 160 concurrent streams, Desk still serving. Exits non-zero if the Desk stalls. | WO-032 |
| `pnpm desk-load` | Desk **concurrent page throughput** (~135 req/s at 50 concurrent, DB-bound) and the SSE/page contention question. Modes: `sweep`, `contention`, or both. | WO-035 |

Each exits non-zero on failure, so they can be chained or run in CI.

## The control that must stay runnable

`sse-bench` is only meaningful because its failure mode was *demonstrated*.
Build the Desk with the never-shipped control feature and watch it stall:

```bash
cd frust-desk && cargo build --features naive-blocking-sse   # std::thread::sleep
# then: pnpm sse-bench  ->  streams stall, ordinary requests time out, exit 1
cd frust-desk && cargo build                                  # restore the real build
```

A metric you have never seen fail is not yet a metric. Keep this reproducible.

## Perf-run hygiene (non-negotiable for the benches)

`sse-bench` and `desk-load` are perf measurements. Per standing rule:

- **A dedicated scratch data-dir** — never point them at the live dev store.
  Start surreal on e.g. `surrealkv://D:/Dev/rust/frust-skel/<wo>-load`, and
  delete it afterwards. A churned store gives pessimistic, confounded numbers.
- A fresh namespace/database needs creating before the kernel will boot:
  `DEFINE NAMESPACE frust; USE NS frust; DEFINE DATABASE skeleton;`
- **Quiet machine.** State what else was running rather than pretending.
- Keep-alive is already built into the drivers: without it, hundreds of clients
  exhaust Windows ephemeral ports and you measure the load generator, not Frust.

## Related
`04 Build Log/2026-07-28 WO-031/032/035 …` in the vault · `v2.0 Deployability Gate`
