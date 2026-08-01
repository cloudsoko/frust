[CmdletBinding()]
param(
    [ValidateSet("doctor", "bootstrap", "build-artifacts")]
    [string]$Command = "doctor"
)

$ErrorActionPreference = "Continue"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$WasmRoot = Join-Path $RepoRoot "wasm-spike"
$ArtifactLockPath = Join-Path $WasmRoot "artifacts.lock.json"
$ArtifactLock = Get-Content -Raw -LiteralPath $ArtifactLockPath | ConvertFrom-Json
$WasmPackage = Get-Content -Raw -LiteralPath (Join-Path $WasmRoot "package.json") | ConvertFrom-Json
$RustVersion = $ArtifactLock.rust
$WasmTarget = $ArtifactLock.target
$RustBuilderImage = $ArtifactLock.builders.rust
$NodeBuilderImage = $ArtifactLock.builders.node
$PnpmVersion = $WasmPackage.packageManager.Split("@")[1]
$SurrealVersion = "3.2.0"
$script:Failures = 0

function Get-PinnedToolchain {
    $stableVersion = (& rustup run stable rustc --version 2>$null) -join ""
    if ($LASTEXITCODE -eq 0 -and $stableVersion -match "^rustc $([regex]::Escape($RustVersion))\b") {
        return "stable"
    }

    $toolchains = (& rustup toolchain list 2>$null) -join "`n"
    if ($toolchains -match "(?m)^$([regex]::Escape($RustVersion))-") {
        return $RustVersion
    }

    return $null
}

function Write-Check {
    param(
        [bool]$Ok,
        [string]$Name,
        [string]$Detail,
        [string]$Remediation = ""
    )

    if ($Ok) {
        Write-Host "[ok]   $Name - $Detail" -ForegroundColor Green
        return
    }

    $script:Failures++
    Write-Host "[fail] $Name - $Detail" -ForegroundColor Red
    if ($Remediation) {
        Write-Host "       fix: $Remediation" -ForegroundColor Yellow
    }
}

function Invoke-Native {
    param(
        [string]$File,
        [string[]]$Arguments,
        [string]$FailureMessage
    )

    & $File @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$FailureMessage (exit $LASTEXITCODE)"
    }
}

function Build-ArtifactsInContainers {
    if (-not (Get-Command docker -ErrorAction SilentlyContinue)) {
        throw "Docker is required to build canonical runtime artifacts on non-Linux hosts"
    }
    if (-not $RustBuilderImage -or -not $NodeBuilderImage) {
        throw "wasm-spike/artifacts.lock.json must pin the Rust and Node builder images"
    }

    $repoMount = "${RepoRoot}:/frust-source"
    $rustBuild = @"
rustup target add $WasmTarget &&
RUSTFLAGS='--remap-path-prefix=/frust-source=/frust-source' cargo build --locked --release --target $WasmTarget --manifest-path wasm-spike/script-engine/Cargo.toml &&
RUSTFLAGS='--remap-path-prefix=/frust-source=/frust-source' cargo build --locked --release --target $WasmTarget --manifest-path wasm-spike/plugin-demo/Cargo.toml &&
mkdir -p wasm-spike/artifacts &&
cp wasm-spike/script-engine/target/$WasmTarget/release/script_engine.wasm wasm-spike/artifacts/script_engine.wasm &&
cp wasm-spike/plugin-demo/target/$WasmTarget/release/plugin_demo.wasm wasm-spike/artifacts/plugin_demo.wasm
"@
    Invoke-Native docker @(
        "run", "--rm",
        "--volume", $repoMount,
        "--mount", "type=volume,source=frust-script-engine-target,target=/frust-source/wasm-spike/script-engine/target",
        "--mount", "type=volume,source=frust-plugin-demo-target,target=/frust-source/wasm-spike/plugin-demo/target",
        "--workdir", "/frust-source",
        $RustBuilderImage,
        "bash", "-c", $rustBuild
    ) "containerized Rust artifact build failed"

    $nodeBuild = @"
corepack enable &&
corepack install --global pnpm@$PnpmVersion &&
pnpm --dir wasm-spike install --frozen-lockfile &&
mkdir -p .frust/build/browser-engine &&
pnpm --dir wasm-spike exec jco transpile /frust-source/wasm-spike/artifacts/script_engine.wasm -o /frust-source/.frust/build/browser-engine --quiet &&
cp .frust/build/browser-engine/script_engine.core.wasm frust-desk/assets/engine/script_engine.core.wasm &&
cp .frust/build/browser-engine/script_engine.js frust-desk/assets/engine/script_engine.js
"@
    Invoke-Native docker @(
        "run", "--rm",
        "--env", "CI=true",
        "--volume", $repoMount,
        "--mount", "type=volume,source=frust-wasm-node-modules,target=/frust-source/wasm-spike/node_modules",
        "--workdir", "/frust-source",
        $NodeBuilderImage,
        "bash", "-c", $nodeBuild
    ) "containerized browser artifact build failed"
}

function Test-ArtifactSet {
    foreach ($property in $ArtifactLock.artifacts.PSObject.Properties) {
        $relative = $property.Name
        $expected = $property.Value
        $path = Join-Path $RepoRoot ($relative -replace "/", [IO.Path]::DirectorySeparatorChar)
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
            Write-Check $false $relative "missing" "pwsh ./scripts/frust.ps1 build-artifacts"
            continue
        }

        $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
        Write-Check ($actual -eq $expected) $relative $(
            if ($actual -eq $expected) { "sha256 verified" } else { "checksum mismatch ($actual)" }
        ) "pwsh ./scripts/frust.ps1 build-artifacts; if the source change is intentional, review and update artifacts.lock.json"
    }
}

function Invoke-Doctor {
    $script:Failures = 0
    Write-Host "Frust doctor ($RepoRoot)"

    $git = Get-Command git -ErrorAction SilentlyContinue
    Write-Check ($null -ne $git) "git" $(if ($git) { "available" } else { "not found" }) "install Git and reopen the shell"

    $rustup = Get-Command rustup -ErrorAction SilentlyContinue
    Write-Check ($null -ne $rustup) "rustup" $(if ($rustup) { "available" } else { "not found" }) "install rustup from https://rustup.rs"
    if ($rustup) {
        $installedToolchain = Get-PinnedToolchain
        $hasRust = $null -ne $installedToolchain
        Write-Check $hasRust "Rust $RustVersion" $(if ($hasRust) { "installed" } else { "not installed" }) "pwsh ./scripts/frust.ps1 bootstrap"

        $targets = if ($installedToolchain) { (& rustup target list --installed --toolchain $installedToolchain 2>$null) -join "`n" } else { "" }
        $hasTarget = $targets -match "(?m)^$([regex]::Escape($WasmTarget))$"
        Write-Check $hasTarget $WasmTarget $(if ($hasTarget) { "installed for Rust $RustVersion" } else { "not installed for Rust $RustVersion" }) "pwsh ./scripts/frust.ps1 bootstrap"
    }

    $pnpm = Get-Command pnpm -ErrorAction SilentlyContinue
    $actualPnpm = if ($pnpm) { ((& pnpm --version) -join "").Trim() } else { "" }
    Write-Check ($null -ne $pnpm -and $actualPnpm -eq $PnpmVersion) "pnpm $PnpmVersion" $(if ($pnpm) { "found $actualPnpm" } else { "not found" }) "enable Corepack and run: corepack install --global pnpm@$PnpmVersion"

    if ($git) {
        $submodules = (& git -C $RepoRoot submodule status --recursive 2>$null) -join "`n"
        $submoduleExit = $LASTEXITCODE
        $unready = @($submodules -split "`n" | Where-Object { $_ -match "^[-+]" })
        $submodulesReady = $submoduleExit -eq 0 -and $unready.Count -eq 0
        Write-Check $submodulesReady "submodules" $(if ($submodulesReady) { "initialized at recorded commits" } else { "missing or not at recorded commit: $($unready -join '; ')" }) "git submodule update --init --recursive"
    }

    Test-ArtifactSet

    try {
        $health = Invoke-WebRequest -Uri "http://127.0.0.1:8899/health" -TimeoutSec 2 -UseBasicParsing
        $version = (Invoke-WebRequest -Uri "http://127.0.0.1:8899/version" -TimeoutSec 2 -UseBasicParsing).Content.Trim()
        $storeReady = $health.StatusCode -eq 200 -and $version -eq "surrealdb-$SurrealVersion"
        Write-Check $storeReady "SurrealDB $SurrealVersion" $(if ($storeReady) { "healthy on 127.0.0.1:8899" } else { "found $version on 127.0.0.1:8899" }) "start the pinned SurrealDB version with the command documented in README.md"
    } catch {
        $surreal = Get-Command surreal -ErrorAction SilentlyContinue
        $docker = Get-Command docker -ErrorAction SilentlyContinue
        $available = if ($surreal) { "SurrealDB CLI is available" } elseif ($docker) { "Docker is available" } else { "no SurrealDB CLI or Docker found" }
        Write-Check $false "SurrealDB $SurrealVersion" "not healthy on 127.0.0.1:8899; $available" "start SurrealDB $SurrealVersion with the command documented in README.md"
    }

    if ($script:Failures -gt 0) {
        Write-Host "`n$($script:Failures) readiness check(s) failed." -ForegroundColor Red
        return 1
    }

    Write-Host "`nFrust prerequisites and runtime artifacts are ready." -ForegroundColor Green
    return 0
}

function Build-Artifacts {
    Build-ArtifactsInContainers
    $script:Failures = 0
    Test-ArtifactSet
    if ($script:Failures -gt 0) {
        throw "generated artifacts do not match wasm-spike/artifacts.lock.json"
    }
}

function Invoke-Bootstrap {
    if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
        throw "git is missing; install Git and reopen the shell"
    }
    if (-not (Get-Command rustup -ErrorAction SilentlyContinue)) {
        throw "rustup is missing; install it from https://rustup.rs"
    }

    Invoke-Native git @("-C", $RepoRoot, "submodule", "update", "--init", "--recursive") "submodule initialization failed"
    $installedToolchain = Get-PinnedToolchain
    if (-not $installedToolchain) {
        Invoke-Native rustup @("toolchain", "install", $RustVersion, "--profile", "minimal", "--component", "clippy", "--component", "rustfmt") "Rust $RustVersion installation failed"
        $installedToolchain = $RustVersion
    }
    Invoke-Native rustup @("target", "add", $WasmTarget, "--toolchain", $installedToolchain) "$WasmTarget installation failed"
    Build-Artifacts

    $cargoToolchain = "+$installedToolchain"
    Invoke-Native cargo @($cargoToolchain, "build", "--locked", "--manifest-path", (Join-Path $RepoRoot "frust-kernel/Cargo.toml")) "kernel build failed"
    Invoke-Native cargo @($cargoToolchain, "build", "--locked", "--manifest-path", (Join-Path $RepoRoot "frust-desk/Cargo.toml")) "Desk build failed"

    Write-Host "`nBootstrap complete. Start SurrealDB, then the kernel and Desk using README.md." -ForegroundColor Green
}

try {
    switch ($Command) {
        "doctor" {
            exit (Invoke-Doctor)
        }
        "build-artifacts" {
            Build-Artifacts
            Write-Host "Runtime artifacts rebuilt and verified." -ForegroundColor Green
        }
        "bootstrap" {
            Invoke-Bootstrap
        }
    }
} catch {
    Write-Host "[fail] $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
