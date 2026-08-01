# Test lanes

`scripts/test.ps1` keeps service-free checks separate from tests that need a
real SurrealDB process. Run it from the repository root:

```powershell
./scripts/test.ps1 -Lane check
./scripts/test.ps1 -Lane offline
./scripts/test.ps1 -Lane live
./scripts/test.ps1 -Lane perf
./scripts/test.ps1 -Lane all
```

Use `-List` with any lane to print its selected targets without starting Cargo
or Docker. `-TimeoutSeconds` bounds each Cargo command, and `-TestThreads`
controls Rust test concurrency. Build output goes to `test/cargo-target` by
default instead of any developer target directory.

The `live` and `perf` lanes refuse to start when `127.0.0.1:8899` is occupied.
When it is free, the runner owns a randomly named, in-memory
`surrealdb/surrealdb:v3.2.0` container and waits at most 45 seconds for its
health endpoint. Cleanup addresses only the container ID returned by that run.
Cargo failures, timeouts, and Ctrl+C all pass through the same cleanup block.

## Classification

`lanes.json` is the reviewable policy. The runner discovers Rust integration
targets from `frust-kernel/kernel/tests` and validates every policy reference
before executing a lane. New integration targets join the live lane by default.

The offline lane runs the kernel and app-SDK library tests plus the listed
service-free integration targets. `frust-orm` currently mixes pure tests and
live HTTP tests in one Rust library test binary, so its full library target runs
in the live lane.

The default live lane does not opt into ignored tests. Named ignored
measurements that work against a fresh database belong to the perf lane.
The policy records tests that remain non-hermetic, including the two seeded
port-8898 scale proofs, the ambient-process restart drill, and the restore test
that invokes a checkout-specific Windows executable.
