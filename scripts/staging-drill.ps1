[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $Tag,

    [Parameter(Mandatory = $true)]
    [string] $Commit,

    [Parameter(Mandatory = $true)]
    [string] $BaselineCommit,

    [Parameter(Mandatory = $true)]
    [string] $BaselineCheckout,

    [Parameter(Mandatory = $true)]
    [string] $PayloadDirectory,

    [Parameter(Mandatory = $true)]
    [string] $EvidenceDirectory,

    [string] $ProjectName = "frust-rc-$PID",
    [int] $DeskPort = 3300
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$root = Split-Path -Parent $PSScriptRoot
$deploy = Join-Path $root "deploy"
$composeFile = Join-Path $deploy "compose.yaml"
$baseline = (Resolve-Path -LiteralPath $BaselineCheckout).Path
$payload = (Resolve-Path -LiteralPath $PayloadDirectory).Path
$evidencePath = [System.IO.Path]::GetFullPath($EvidenceDirectory)
$safeTag = $Tag -replace '[^A-Za-z0-9_.-]', '-'
$safeProject = $ProjectName.ToLowerInvariant() -replace '[^a-z0-9_-]', '-'
$candidateKernel = "frust/kernel:$safeTag-$safeProject"
$candidateDesk = "frust/desk:$safeTag-$safeProject"
$baselineKernel = "frust/kernel:baseline-$safeProject"
$baselineDesk = "frust/desk:baseline-$safeProject"
$databaseImage = "frust/surrealdb:3.2.3-$safeProject"
$tenant = "staging"
$namespace = "frust"
$started = [DateTimeOffset]::UtcNow
$timings = [ordered]@{}
$checks = [ordered]@{}
$reports = [ordered]@{}
$status = "failed"
$failure = $null

New-Item -ItemType Directory -Force -Path $evidencePath | Out-Null

$candidateArtifactLock = Get-FileHash (Join-Path $root "wasm-spike/artifacts.lock.json") -Algorithm SHA256
$baselineArtifactLock = Get-FileHash (Join-Path $baseline "wasm-spike/artifacts.lock.json") -Algorithm SHA256
if ($candidateArtifactLock.Hash -ne $baselineArtifactLock.Hash) {
    throw "baseline and candidate runtime artifact locks differ; build the baseline artifacts separately"
}

function Invoke-Docker {
    param([Parameter(Mandatory = $true)][string[]] $Arguments)
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & docker @Arguments
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($exitCode -ne 0) {
        throw "docker $($Arguments -join ' ') failed with exit code $exitCode"
    }
}

function Invoke-DockerCapture {
    param([Parameter(Mandatory = $true)][string[]] $Arguments)
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = & docker @Arguments 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($exitCode -ne 0) {
        throw "docker $($Arguments -join ' ') failed: $($output -join [Environment]::NewLine)"
    }
    return ($output -join [Environment]::NewLine)
}

function Invoke-Compose {
    param([Parameter(Mandatory = $true)][string[]] $Arguments)
    Invoke-Docker -Arguments (@("compose", "-p", $ProjectName, "-f", $composeFile) + $Arguments)
}

function Invoke-ComposeCapture {
    param([Parameter(Mandatory = $true)][string[]] $Arguments)
    return Invoke-DockerCapture -Arguments (@("compose", "-p", $ProjectName, "-f", $composeFile) + $Arguments)
}

function Set-Images {
    param([Parameter(Mandatory = $true)][string] $Kernel, [Parameter(Mandatory = $true)][string] $Desk)
    $env:FRUST_KERNEL_IMAGE = $Kernel
    $env:FRUST_DESK_IMAGE = $Desk
}

function Wait-Staging {
    param([int] $Attempts = 90)
    for ($attempt = 1; $attempt -le $Attempts; $attempt++) {
        & docker compose -p $ProjectName -f $composeFile exec -T kernel `
            curl --fail --silent http://127.0.0.1:8790/ready *> $null
        $kernelReady = $LASTEXITCODE -eq 0
        try {
            $response = Invoke-WebRequest -UseBasicParsing -Uri "http://127.0.0.1:$DeskPort/admission" -TimeoutSec 3
            $deskReady = $response.StatusCode -eq 200
        }
        catch {
            $deskReady = $false
        }
        if ($kernelReady -and $deskReady) {
            return
        }
        Start-Sleep -Seconds 2
    }
    throw "staging services did not become ready after $Attempts attempts"
}

function Invoke-Sql {
    param([Parameter(Mandatory = $true)][string] $Sql)
    $user = (Get-Content -LiteralPath (Join-Path $deploy "secrets/surreal_root_username.txt") -Raw).Trim()
    $pass = (Get-Content -LiteralPath (Join-Path $deploy "secrets/surreal_root_password.txt") -Raw).Trim()
    $arguments = @(
        "compose", "-p", $ProjectName, "-f", $composeFile,
        "exec", "-T",
        "-e", "SURREAL_USER=$user",
        "-e", "SURREAL_PASS=$pass",
        "-e", "SURREAL_AUTH_LEVEL=root",
        "database", "surreal", "sql",
        "--endpoint", "http://127.0.0.1:8000",
        "--namespace", $namespace,
        "--database", $tenant,
        "--hide-welcome"
    )
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = $Sql | & docker @arguments 2>&1
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousPreference
    }
    if ($exitCode -ne 0) {
        throw "staging SQL failed: $($output -join [Environment]::NewLine)"
    }
    return ($output -join [Environment]::NewLine)
}

function Assert-Marker {
    param([Parameter(Mandatory = $true)][string] $Expected)
    $actual = Invoke-Sql "SELECT VALUE marker FROM ONLY dr_probe:release;"
    if ($actual -notmatch [regex]::Escape($Expected)) {
        throw "expected marker $Expected, query returned: $actual"
    }
}

function Measure-Step {
    param([Parameter(Mandatory = $true)][string] $Name, [Parameter(Mandatory = $true)][scriptblock] $Operation)
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        & $Operation
    }
    finally {
        $watch.Stop()
        $timings[$Name] = $watch.ElapsedMilliseconds
    }
}

try {
    $env:COMPOSE_PROJECT_NAME = $ProjectName
    $env:FRUST_BIND_IP = "127.0.0.1"
    $env:FRUST_DESK_PORT = "$DeskPort"
    $env:FRUST_TENANTS = $tenant
    $env:FRUST_NS = $namespace
    $env:FRUST_VERSION = $Tag.TrimStart("v")
    $env:VCS_REF = $Commit
    $env:FRUST_SURREAL_IMAGE = $databaseImage

    & (Join-Path $deploy "init-secrets.ps1")

    Measure-Step "build_candidate_kernel_ms" {
        Invoke-Docker -Arguments @(
            "build", "--pull",
            "--build-context", "payload=$payload",
            "--build-arg", "FRUST_VERSION=$($env:FRUST_VERSION)",
            "--build-arg", "VCS_REF=$Commit",
            "-f", (Join-Path $deploy "Dockerfile.kernel.release"),
            "-t", $candidateKernel,
            $deploy
        )
    }
    Measure-Step "build_candidate_desk_ms" {
        Invoke-Docker -Arguments @(
            "build", "--pull",
            "--build-context", "payload=$payload",
            "--build-arg", "FRUST_VERSION=$($env:FRUST_VERSION)",
            "--build-arg", "VCS_REF=$Commit",
            "-f", (Join-Path $deploy "Dockerfile.desk.release"),
            "-t", $candidateDesk,
            $deploy
        )
    }
    Measure-Step "build_database_ms" {
        Invoke-Docker -Arguments @(
            "build", "--pull",
            "--build-context", "legal=$root",
            "--build-arg", "VCS_REF=$Commit",
            "-f", (Join-Path $deploy "Dockerfile.surrealdb"),
            "-t", $databaseImage,
            $deploy
        )
    }
    Measure-Step "build_baseline_kernel_ms" {
        Invoke-Docker -Arguments @(
            "build", "--pull",
            "--build-context", "artifacts=$(Join-Path $payload 'runtime')",
            "--build-context", "deploy=$(Join-Path $baseline 'deploy')",
            "--build-arg", "FRUST_VERSION=baseline",
            "--build-arg", "VCS_REF=$BaselineCommit",
            "-f", (Join-Path $baseline "deploy/Dockerfile.kernel"),
            "-t", $baselineKernel,
            (Join-Path $baseline "frust-kernel")
        )
    }
    Measure-Step "build_baseline_desk_ms" {
        Invoke-Docker -Arguments @(
            "build", "--pull",
            "--build-context", "topcoat=$(Join-Path $baseline 'topcoat')",
            "--build-arg", "FRUST_VERSION=baseline",
            "--build-arg", "VCS_REF=$BaselineCommit",
            "-f", (Join-Path $baseline "deploy/Dockerfile.desk"),
            "-t", $baselineDesk,
            (Join-Path $baseline "frust-desk")
        )
    }

    Set-Images -Kernel $baselineKernel -Desk $baselineDesk
    Measure-Step "baseline_deploy_ms" {
        Invoke-Compose -Arguments @("up", "-d", "--no-build")
        Wait-Staging
    }
    $checks["baseline_ready"] = $true
    Invoke-Sql "DEFINE TABLE dr_probe SCHEMALESS PERMISSIONS FULL; UPSERT dr_probe:release SET marker = 'before-backup';" | Out-Null
    Assert-Marker "before-backup"
    $checks["baseline_data_seeded"] = $true

    Set-Images -Kernel $candidateKernel -Desk $candidateDesk
    Measure-Step "upgrade_to_candidate_ms" {
        Invoke-Compose -Arguments @("--profile", "operations", "run", "--rm", "migrate")
        Invoke-Compose -Arguments @("up", "-d", "--no-build", "--force-recreate", "kernel", "desk")
        Wait-Staging
    }
    Assert-Marker "before-backup"
    $checks["upgrade_preserved_data"] = $true

    Set-Images -Kernel $baselineKernel -Desk $baselineDesk
    Measure-Step "rollback_to_baseline_ms" {
        Invoke-Compose -Arguments @("up", "-d", "--no-build", "--force-recreate", "kernel", "desk")
        Wait-Staging
    }
    Assert-Marker "before-backup"
    $checks["rollback_preserved_data"] = $true

    Set-Images -Kernel $candidateKernel -Desk $candidateDesk
    Measure-Step "redeploy_candidate_ms" {
        Invoke-Compose -Arguments @("up", "-d", "--no-build", "--force-recreate", "kernel", "desk")
        Wait-Staging
    }

    Measure-Step "backup_ms" {
        $reports["backup"] = Invoke-ComposeCapture -Arguments @(
            "run", "--rm", "--no-deps", "kernel",
            "backup", "--tenant", $tenant,
            "--output", "/var/lib/frust/recovery/baseline"
        )
    }
    $reports["verify"] = Invoke-ComposeCapture -Arguments @(
        "run", "--rm", "--no-deps", "kernel",
        "backup", "verify", "--input", "/var/lib/frust/recovery/baseline"
    )
    Invoke-Sql "UPDATE dr_probe:release SET marker = 'after-backup';" | Out-Null
    Assert-Marker "after-backup"

    Invoke-Compose -Arguments @("stop", "desk", "kernel")
    Measure-Step "restore_ms" {
        $reports["restore"] = Invoke-ComposeCapture -Arguments @(
            "run", "--rm", "--no-deps", "kernel",
            "restore", "--tenant", $tenant,
            "--input", "/var/lib/frust/recovery/baseline",
            "--safety-backup", "/var/lib/frust/recovery/pre-restore",
            "--confirm-drop", "$namespace/$tenant"
        )
    }
    $reports["manifest"] = Invoke-ComposeCapture -Arguments @(
        "run", "--rm", "--no-deps", "--entrypoint", "cat", "kernel",
        "/var/lib/frust/recovery/baseline/manifest.json"
    )
    Invoke-Compose -Arguments @("up", "-d", "--no-build", "kernel", "desk")
    Wait-Staging
    Assert-Marker "before-backup"
    $checks["restore_recovered_backup_state"] = $true
    $checks["final_candidate_ready"] = $true
    $status = "passed"
}
catch {
    $failure = $_.Exception.Message
    throw
}
finally {
    try {
        $logs = Invoke-ComposeCapture -Arguments @("logs", "--no-color")
        [System.IO.File]::WriteAllText((Join-Path $evidencePath "compose.log"), $logs + [Environment]::NewLine)
    }
    catch {
        [System.IO.File]::WriteAllText(
            (Join-Path $evidencePath "compose-log-error.txt"),
            $_.Exception.Message + [Environment]::NewLine
        )
    }

    $finished = [DateTimeOffset]::UtcNow
    $record = [ordered]@{
        schema_version = 1
        status = $status
        environment = "ephemeral-staging"
        tag = $Tag
        commit = $Commit
        baseline_commit = $BaselineCommit
        project = $ProjectName
        started_at = $started.ToString("o")
        finished_at = $finished.ToString("o")
        elapsed_ms = [int64]($finished - $started).TotalMilliseconds
        images = [ordered]@{
            candidate_kernel = $candidateKernel
            candidate_desk = $candidateDesk
            baseline_kernel = $baselineKernel
            baseline_desk = $baselineDesk
            database = $databaseImage
        }
        timings_ms = $timings
        checks = $checks
        recovery_reports = $reports
        error = $failure
    }
    $json = $record | ConvertTo-Json -Depth 12
    [System.IO.File]::WriteAllText(
        (Join-Path $evidencePath "staging-drill.json"),
        $json + [Environment]::NewLine
    )
    try {
        Invoke-Compose -Arguments @("down", "--volumes", "--remove-orphans")
    }
    catch {
        Write-Warning "staging cleanup failed: $($_.Exception.Message)"
    }
}
