# Backup, Restore, and Release Rollback Runbook

This runbook applies to the supported single-host `database-per-tenant`
deployment in `deploy/compose.yaml`. It does not authorize a tenant restore for
a shared database topology; the recovery command refuses that operation.

## Recovery objectives

Every exercise records actual durations. Until representative production data
has been measured, use these as drill targets rather than production promises:

- backup target: 15 minutes;
- restore target: 30 minutes;
- release rollback target: 15 minutes;
- recovery point: the timestamp in the selected backup manifest.

## Before a release

1. Record the running image identifiers and Git commit.
2. Confirm the target tenant and its exact namespace/database scope.
3. Create and verify a backup while the current release is still serving.
4. Confirm there is enough space for the backup and the restore safety backup.
5. Keep the previous immutable images available until the rollout is accepted.

```powershell
$tenant = "site"
$scope = "frust/site"
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"

docker compose run --rm --no-deps kernel `
  backup --tenant $tenant --output "/var/lib/frust/recovery/pre-release-$stamp"
docker compose run --rm --no-deps kernel `
  backup verify --input "/var/lib/frust/recovery/pre-release-$stamp"
```

Copy completed backups to encrypted off-host storage according to the target
environment's retention policy. A Compose volume alone is not an off-host
backup.

## Upgrade and rollback

Run metadata migration explicitly, then recreate only the application services:

```powershell
docker compose --profile operations run --rm migrate
docker compose up -d --no-build --force-recreate kernel desk
docker compose ps
docker compose exec kernel curl --fail --silent http://127.0.0.1:8790/ready
docker compose exec desk curl --fail --silent http://127.0.0.1:3000/admission
```

If acceptance fails, restore the previously recorded immutable image values in
`.env`, recreate `kernel` and `desk`, and repeat the health and data checks.
Database schema rollback is not inferred from an image rollback. If the
migration changed stored data incompatibly, use the verified pre-release backup
and the restore procedure below.

## Tenant restore

Stop writers before the destructive reset. The restore command verifies the
source again after creating a safety backup, requires the exact scope, rotates
restored access keys, and runs the post-restore keyguard.

```powershell
$tenant = "site"
$scope = "frust/site"
$source = "/var/lib/frust/recovery/pre-release-YYYYMMDD-HHMMSS"
$safety = "/var/lib/frust/recovery/pre-restore-YYYYMMDD-HHMMSS"

docker compose stop desk kernel
docker compose run --rm --no-deps kernel `
  restore --tenant $tenant --input $source --safety-backup $safety `
  --confirm-drop $scope
docker compose up -d --no-build kernel desk
docker compose exec kernel curl --fail --silent http://127.0.0.1:8790/ready
docker compose exec desk curl --fail --silent http://127.0.0.1:3000/admission
```

Do not resume traffic if restore reports an import, key rotation, or keyguard
failure. Preserve the named safety backup and logs before any further action.

## Required evidence

Retain the release tag and commit, image identifiers, backup manifest and
checksum, start/end timestamps, measured durations, health results, data
sentinel results, and sanitized Compose logs. `scripts/staging-drill.ps1`
generates this evidence for the release candidate's isolated staging exercise.
