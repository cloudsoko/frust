# Frust deployment foundation

This directory packages the current Frust kernel, Desk, and the behavior-pinned
SurrealDB 3.2.0 release into a production-like, single-host deployment. It is a
foundation for evaluation and controlled pilots, not a claim that Frust has a
complete production operations story.

## What this provides

- multi-stage OCI builds with pinned Rust and SurrealDB versions;
- non-root kernel, Desk, and database processes;
- file-backed Docker secrets instead of credentials in Compose or `.env`;
- an internal datastore network with no host-published database or kernel port;
- a Desk port bound to loopback by default;
- persistent SurrealKV and mail-outbox volumes;
- health-gated database initialization, kernel boot, and Desk startup;
- read-only application filesystems, dropped capabilities, and bounded logs;
- an explicit one-shot metadata migration command.

The initializer intentionally supports only `database-per-tenant`, the default
topology. It refuses another topology rather than guessing at its provisioning
unit. The kernel itself retains its other supported tenancy strategies.

## Prerequisites

- Docker Engine with Compose v2 and BuildKit support;
- enough memory and disk for Rust/Wasmtime release builds;
- the hook artifacts in `../wasm-spike/artifacts`.

The artifact files are copied into the kernel image. Their source is not made
reproducible by this deployment layer; a release pipeline must build and attest
them before the image build.

## Start

From this directory:

```powershell
Copy-Item .env.example .env
.\init-secrets.ps1
docker compose config --quiet
docker compose build
docker compose up -d
docker compose ps
```

On Unix, use `./init-secrets.sh`. Desk is available at
`http://127.0.0.1:3000` by default. Change `FRUST_BIND_IP` only when a trusted
TLS reverse proxy or firewall controls the exposed interface.

Observe readiness without publishing the internal services:

```powershell
docker compose exec kernel curl --fail --silent http://127.0.0.1:8790/ready
docker compose exec database surreal isready --endpoint http://127.0.0.1:8000
docker compose exec desk curl --fail --silent http://127.0.0.1:3000/admission
docker compose logs --follow kernel desk database
```

## Configuration and secrets

`init-secrets` creates ignored files under `secrets/`. Compose mounts them at
`/run/secrets`; the entrypoints read them without printing their contents. Both
the database and kernel refuse the development password `root`.

Runtime settings live in `.env`, using `.env.example` as the contract. The
deployment deliberately keeps root database credentials out of environment
variables in Compose. They exist in the child process environment only after
the kernel entrypoint reads the mounted secret because the current kernel
connection type accepts strings.

`FRUST_TENANTS` is a comma-separated set of plain identifiers. The initializer
creates each as a database under `FRUST_NS`, idempotently, before the kernel
boots. Invalid identifiers and unsupported topologies fail the initializer.

## Metadata migrations

Normal startup does not pass `--accept-meta-migrations`. Inspect the planned
change and take a verified backup before explicitly running:

```powershell
docker compose --profile operations run --rm migrate
docker compose up -d kernel desk
```

This is an operator-controlled acceptance switch, not a rollback mechanism.

## Shutdown and persistence

Use `docker compose down` for an orderly stop. Topcoat handles `SIGTERM` and
allows in-flight Desk requests to finish. SurrealDB receives a 30-second stop
window. The current kernel uses a blocking HTTP server and does not implement a
graceful-drain signal; Compose gives it 15 seconds, after which Docker may send
`SIGKILL`. Do not claim zero-loss kernel shutdown until that runtime gap is
closed.

`docker compose down` preserves the named volumes. `docker compose down -v`
deletes the database and outbox and is therefore destructive.

## Production boundaries

Before exposing this outside a controlled pilot:

1. Publish immutable images by digest with SBOMs, signatures, and provenance.
2. Put Desk behind a TLS reverse proxy and define trusted proxy/header policy.
3. Replace file mail or configure a supported authenticated/TLS SMTP transport.
4. Implement and rehearse verified off-host backup and restore procedures.
5. Add kernel signal handling and a measured request-drain deadline.
6. Define CPU, memory, file-descriptor, and volume budgets for the target host.
7. Send structured logs and metrics to durable external systems.

No Kubernetes or systemd manifests are included because their rollout,
storage, and recovery behavior has not been verified for this repository.
