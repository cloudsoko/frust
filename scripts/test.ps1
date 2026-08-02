[CmdletBinding()]
param(
    [ValidateSet("offline", "smoke", "live", "perf", "all", "check")]
    [string] $Lane = "offline",

    [ValidateRange(30, 3600)]
    [int] $TimeoutSeconds = 600,

    [ValidateRange(1, 32)]
    [int] $TestThreads = 2,

    [string] $CargoTargetDir,

    [switch] $List
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$KernelRoot = Join-Path $RepoRoot "frust-kernel"
$KernelTests = Join-Path $KernelRoot "kernel/tests"
$PolicyPath = Join-Path $RepoRoot "test/lanes.json"

if (-not $CargoTargetDir) {
    $CargoTargetDir = Join-Path $RepoRoot "test/cargo-target"
}
$CargoTargetDir = [System.IO.Path]::GetFullPath($CargoTargetDir)

function Convert-ToArgumentString {
    param([string[]] $Arguments)

    $quoted = foreach ($argument in $Arguments) {
        if ($argument -notmatch '[\s"]') {
            $argument
            continue
        }
        '"' + ($argument -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
    }
    return $quoted -join " "
}

function Stop-OwnedProcessTree {
    param([System.Diagnostics.Process] $Process)

    if ($Process.HasExited) {
        return
    }

    try {
        $killTree = $Process.GetType().GetMethod("Kill", [type[]] @([bool]))
        if ($null -ne $killTree) {
            $killTree.Invoke($Process, @($true))
        } elseif ($env:OS -eq "Windows_NT") {
            & taskkill.exe /PID $Process.Id /T /F 2>&1 | Out-Null
        } else {
            $Process.Kill()
        }
        $Process.WaitForExit(5000) | Out-Null
    } catch {
        Write-Warning "Could not stop owned process tree PID $($Process.Id): $($_.Exception.Message)"
    }
}

function Invoke-BoundedProcess {
    param(
        [Parameter(Mandatory)] [string] $FilePath,
        [Parameter(Mandatory)] [string[]] $Arguments,
        [Parameter(Mandatory)] [int] $Timeout,
        [string] $WorkingDirectory = $RepoRoot,
        [switch] $AllowFailure,
        [switch] $Quiet
    )

    $stdoutPath = Join-Path ([System.IO.Path]::GetTempPath()) ("frust-test-out-" + [guid]::NewGuid().ToString("N") + ".log")
    $stderrPath = Join-Path ([System.IO.Path]::GetTempPath()) ("frust-test-err-" + [guid]::NewGuid().ToString("N") + ".log")
    $process = $null

    try {
        $start = [System.Diagnostics.ProcessStartInfo]::new()
        $start.FileName = $FilePath
        $start.Arguments = Convert-ToArgumentString $Arguments
        $start.WorkingDirectory = $WorkingDirectory
        $start.UseShellExecute = $false
        $start.CreateNoWindow = $true
        $start.RedirectStandardOutput = $true
        $start.RedirectStandardError = $true

        $process = [System.Diagnostics.Process]::new()
        $process.StartInfo = $start
        if (-not $process.Start()) {
            throw "Failed to start $FilePath"
        }

        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($Timeout * 1000)) {
            Stop-OwnedProcessTree $process
            throw "Timed out after ${Timeout}s: $FilePath $($start.Arguments)"
        }
        $stdout = $stdoutTask.GetAwaiter().GetResult()
        $stderr = $stderrTask.GetAwaiter().GetResult()

        [System.IO.File]::WriteAllText($stdoutPath, $stdout)
        [System.IO.File]::WriteAllText($stderrPath, $stderr)
        if (-not $Quiet) {
            if ($stdout) { Write-Host $stdout.TrimEnd() }
            if ($stderr) { Write-Host $stderr.TrimEnd() }
        }

        $result = [pscustomobject]@{
            ExitCode = $process.ExitCode
            StdOut = $stdout
            StdErr = $stderr
        }
        if ($result.ExitCode -ne 0 -and -not $AllowFailure) {
            throw "Command exited $($result.ExitCode): $FilePath $($start.Arguments)"
        }
        return $result
    } finally {
        if ($null -ne $process -and -not $process.HasExited) {
            Stop-OwnedProcessTree $process
        }
        Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
    }
}

function Get-IntegrationTargets {
    return @(
        Get-ChildItem -LiteralPath $KernelTests -Filter "*.rs" -File |
            Where-Object { $_.BaseName -ne "common" } |
            ForEach-Object { $_.BaseName } |
            Sort-Object
    )
}

function Assert-UniqueValues {
    param([string] $Name, [string[]] $Values)

    $duplicates = @($Values | Group-Object | Where-Object Count -gt 1 | ForEach-Object Name)
    if ($duplicates.Count -gt 0) {
        throw "$Name contains duplicates: $($duplicates -join ', ')"
    }
}

function Test-LanePolicy {
    param($Policy, [string[]] $AllTargets)

    if ($Policy.version -ne 1) {
        throw "Unsupported test lane policy version: $($Policy.version)"
    }
    if ($Policy.surreal.image -ne "surrealdb/surrealdb:v3.2.0") {
        throw "The hermetic lane must pin SurrealDB exactly to surrealdb/surrealdb:v3.2.0"
    }
    if ($Policy.surreal.host -ne "127.0.0.1" -or $Policy.surreal.hostPort -ne 8899) {
        throw "Existing tests require the isolated database at 127.0.0.1:8899"
    }

    $offline = @($Policy.offlineTargets | ForEach-Object { [string] $_ })
    $smoke = @($Policy.smokeTargets | ForEach-Object { [string] $_ })
    $liveExcluded = @($Policy.liveExcludedTargets | ForEach-Object { [string] $_.target })
    $perfExclusive = @($Policy.perfExclusiveTargets | ForEach-Object { [string] $_ })
    $liveLibraries = @($Policy.liveLibraryPackages | ForEach-Object { [string] $_ })
    Assert-UniqueValues "offlineTargets" $offline
    Assert-UniqueValues "smokeTargets" $smoke
    Assert-UniqueValues "liveExcludedTargets" $liveExcluded
    Assert-UniqueValues "perfExclusiveTargets" $perfExclusive
    Assert-UniqueValues "liveLibraryPackages" $liveLibraries
    if ("frust-orm" -notin $liveLibraries) {
        throw "frust-orm must remain in the live lane while its cfg(test) testkit calls 127.0.0.1:8899"
    }
    $ormTestkit = Get-Content -LiteralPath (Join-Path $KernelRoot "orm/src/testkit.rs") -Raw
    if ($ormTestkit -notmatch '127\.0\.0\.1:8899') {
        throw "frust-orm testkit topology changed; reclassify its library target before running lanes"
    }

    $referencedTargets = @($offline + $smoke + $liveExcluded + $perfExclusive + @(
        $Policy.perfCases | ForEach-Object { [string] $_.target }
    ) + @(
        $Policy.knownNonHermeticCases | ForEach-Object { [string] $_.target }
    )) | Sort-Object -Unique
    $missingTargets = @($referencedTargets | Where-Object { $_ -notin $AllTargets })
    if ($missingTargets.Count -gt 0) {
        throw "Lane policy names missing integration targets: $($missingTargets -join ', ')"
    }

    $overlap = @($offline | Where-Object { $_ -in $liveExcluded -or $_ -in $perfExclusive })
    if ($overlap.Count -gt 0) {
        throw "Offline targets overlap excluded/perf-exclusive targets: $($overlap -join ', ')"
    }
    $invalidSmoke = @($smoke | Where-Object {
        $_ -in $offline -or $_ -in $liveExcluded -or $_ -in $perfExclusive
    })
    if ($invalidSmoke.Count -gt 0) {
        throw "Smoke targets must belong to the hermetic live lane: $($invalidSmoke -join ', ')"
    }

    foreach ($case in $Policy.perfCases) {
        $path = Join-Path $KernelTests ("$($case.target).rs")
        $source = Get-Content -LiteralPath $path -Raw
        if ($case.mode -eq "ignored-exact") {
            if (-not $case.test) {
                throw "Perf case $($case.target) needs an exact test name"
            }
            $escaped = [regex]::Escape([string] $case.test)
            if ($source -notmatch "(?s)#\[ignore[^\]]*\].{0,500}?fn\s+$escaped\s*\(") {
                throw "Perf case $($case.target)::$($case.test) is not an ignored test"
            }
        } elseif ($case.mode -ne "default") {
            throw "Unknown perf mode '$($case.mode)' for $($case.target)"
        }
    }

    foreach ($case in $Policy.knownNonHermeticCases) {
        if ($case.test -eq "*") { continue }
        $path = Join-Path $KernelTests ("$($case.target).rs")
        $source = Get-Content -LiteralPath $path -Raw
        $escaped = [regex]::Escape([string] $case.test)
        if ($source -notmatch "fn\s+$escaped\s*\(") {
            throw "Known non-hermetic test disappeared: $($case.target)::$($case.test)"
        }
    }

    Write-Host "Lane policy OK: $($AllTargets.Count) integration targets classified."
}

function Get-LiveTargets {
    param($Policy, [string[]] $AllTargets)

    $offline = @($Policy.offlineTargets | ForEach-Object { [string] $_ })
    $excluded = @($Policy.liveExcludedTargets | ForEach-Object { [string] $_.target })
    $perfOnly = @($Policy.perfExclusiveTargets | ForEach-Object { [string] $_ })
    return @($AllTargets | Where-Object {
        $_ -notin $offline -and $_ -notin $excluded -and $_ -notin $perfOnly
    })
}

function Assert-PortAvailable {
    param([string] $HostName, [int] $Port)

    $address = [System.Net.IPAddress]::Parse($HostName)
    $listener = [System.Net.Sockets.TcpListener]::new($address, $Port)
    try {
        $listener.Start()
    } catch {
        throw "Refusing to run: ${HostName}:$Port is already occupied. The runner will not inspect or stop the owning process."
    } finally {
        $listener.Stop()
    }
}

function Start-TestDatabase {
    param($DatabasePolicy)

    Assert-PortAvailable ([string] $DatabasePolicy.host) ([int] $DatabasePolicy.hostPort)
    $docker = (Get-Command docker -ErrorAction Stop).Source
    Invoke-BoundedProcess $docker @("version", "--format", "{{.Server.Version}}") 20 $RepoRoot | Out-Null

    $name = "frust-test-" + [guid]::NewGuid().ToString("N")
    $publish = "$($DatabasePolicy.host):$($DatabasePolicy.hostPort):$($DatabasePolicy.containerPort)"
    $containerId = $null
    try {
        $run = Invoke-BoundedProcess $docker @(
            "run", "--detach", "--rm",
            "--name", $name,
            "--publish", $publish,
            [string] $DatabasePolicy.image,
            "start", "--log", "warn",
            "--user", [string] $DatabasePolicy.user,
            "--pass", [string] $DatabasePolicy.password,
            "memory"
        ) 180 $RepoRoot
        $containerId = $run.StdOut.Trim()
        if ($containerId -notmatch '^[a-f0-9]{12,64}$') {
            throw "Docker returned an invalid container id: '$containerId'"
        }

        $deadline = [DateTime]::UtcNow.AddSeconds([int] $DatabasePolicy.readinessTimeoutSeconds)
        $healthUri = "http://$($DatabasePolicy.host):$($DatabasePolicy.hostPort)/health"
        while ([DateTime]::UtcNow -lt $deadline) {
            try {
                $response = Invoke-WebRequest -Uri $healthUri -UseBasicParsing -TimeoutSec 2
                if ($response.StatusCode -eq 200) {
                    Write-Host "SurrealDB ready: $containerId ($($DatabasePolicy.image))"
                    return [pscustomobject]@{ Docker = $docker; Id = $containerId; Name = $name }
                }
            } catch {
                Start-Sleep -Milliseconds 500
            }
        }

        $logs = Invoke-BoundedProcess $docker @("logs", $containerId) 10 $RepoRoot -AllowFailure -Quiet
        if ($logs.StdOut) { Write-Host $logs.StdOut.TrimEnd() }
        if ($logs.StdErr) { Write-Host $logs.StdErr.TrimEnd() }
        throw "SurrealDB was not ready at $healthUri within $($DatabasePolicy.readinessTimeoutSeconds)s"
    } catch {
        # The caller cannot clean up until this function returns. If startup
        # fails first, remove only the random name allocated by this run.
        try {
            Invoke-BoundedProcess $docker @("rm", "--force", $name) 20 $RepoRoot -AllowFailure -Quiet | Out-Null
        } catch {
            Write-Warning "Could not clean up failed test container '$name': $($_.Exception.Message)"
        }
        throw
    }
}

function Stop-TestDatabase {
    param($Database)

    if ($null -eq $Database) { return }
    try {
        Invoke-BoundedProcess $Database.Docker @("rm", "--force", $Database.Id) 20 $RepoRoot -AllowFailure -Quiet | Out-Null
        Write-Host "Removed owned SurrealDB container $($Database.Id)."
    } catch {
        Write-Warning "Could not remove owned container $($Database.Id): $($_.Exception.Message)"
    }
}

function Invoke-CargoTests {
    param([string[]] $Arguments, [string] $Description)

    $cargo = (Get-Command cargo -ErrorAction Stop).Source
    Write-Host "`n== $Description =="
    Write-Host "cargo $($Arguments -join ' ')"
    if ($List) { return }
    Invoke-BoundedProcess $cargo $Arguments $TimeoutSeconds $KernelRoot | Out-Null
}

$policy = Get-Content -LiteralPath $PolicyPath -Raw | ConvertFrom-Json
$allTargets = @(Get-IntegrationTargets)
Test-LanePolicy $policy $allTargets
$liveTargets = @(Get-LiveTargets $policy $allTargets)
$smokeTargets = @($policy.smokeTargets | ForEach-Object { [string] $_ })

if ($Lane -eq "check") {
    Write-Host "Offline targets: $(@($policy.offlineTargets).Count)"
    Write-Host "Smoke targets: $($smokeTargets.Count)"
    Write-Host "Live targets: $($liveTargets.Count)"
    Write-Host "Live library packages: $(@($policy.liveLibraryPackages).Count)"
    Write-Host "Perf cases: $(@($policy.perfCases).Count)"
    Write-Host "Known non-hermetic cases: $(@($policy.knownNonHermeticCases).Count)"
    exit 0
}

$offlineArgs = @("test", "--locked", "-p", "frust-kernel", "-p", "frust-app-sdk", "--lib")
$offlineIntegrationArgs = @("test", "--locked", "-p", "frust-kernel")
foreach ($target in $policy.offlineTargets) {
    $offlineIntegrationArgs += @("--test", [string] $target)
}
$offlineIntegrationArgs += @("--", "--test-threads=$TestThreads")

$liveArgs = @("test", "--locked", "-p", "frust-kernel")
foreach ($target in $liveTargets) {
    $liveArgs += @("--test", $target)
}
$liveArgs += @("--", "--test-threads=$TestThreads")

$smokeArgs = @("test", "--locked", "-p", "frust-kernel")
foreach ($target in $smokeTargets) {
    $smokeArgs += @("--test", $target)
}
$smokeArgs += @("--", "--test-threads=$TestThreads")

$liveLibraryArgs = @("test", "--locked")
foreach ($package in $policy.liveLibraryPackages) {
    $liveLibraryArgs += @("-p", [string] $package)
}
$liveLibraryArgs += @("--lib", "--", "--test-threads=$TestThreads")

if ($List) {
    Write-Host "`nOffline integration targets:`n  $(@($policy.offlineTargets) -join "`n  ")"
    Write-Host "`nSmoke integration targets:`n  $($smokeTargets -join "`n  ")"
    Write-Host "`nLive integration targets:`n  $($liveTargets -join "`n  ")"
    Write-Host "`nLive library packages:`n  $(@($policy.liveLibraryPackages) -join "`n  ")"
    Write-Host "`nKnown non-hermetic cases (never selected):"
    foreach ($case in $policy.knownNonHermeticCases) {
        Write-Host "  $($case.target)::$($case.test) - $($case.reason)"
    }
    Write-Host "`nLane limitations:"
    foreach ($limitation in $policy.laneLimitations) {
        Write-Host "  $limitation"
    }
}

$savedTargetDir = [Environment]::GetEnvironmentVariable("CARGO_TARGET_DIR", "Process")
$savedEndpoint = [Environment]::GetEnvironmentVariable("FRUST_DB_ENDPOINT", "Process")
$savedUser = [Environment]::GetEnvironmentVariable("FRUST_DB_ROOT_USER", "Process")
$savedPass = [Environment]::GetEnvironmentVariable("FRUST_DB_ROOT_PASS", "Process")
$database = $null

try {
    $env:CARGO_TARGET_DIR = $CargoTargetDir

    if ($Lane -in @("offline", "all")) {
        Invoke-CargoTests $offlineArgs "offline workspace library tests"
        Invoke-CargoTests $offlineIntegrationArgs "offline kernel integration tests"
    }

    if ($Lane -in @("smoke", "live", "perf", "all") -and -not $List) {
        $database = Start-TestDatabase $policy.surreal
        $env:FRUST_DB_ENDPOINT = "http://$($policy.surreal.host):$($policy.surreal.hostPort)"
        $env:FRUST_DB_ROOT_USER = [string] $policy.surreal.user
        $env:FRUST_DB_ROOT_PASS = [string] $policy.surreal.password
    }

    if ($Lane -in @("smoke", "live", "all")) {
        Invoke-CargoTests $liveLibraryArgs "live library tests"
    }

    if ($Lane -eq "smoke") {
        Invoke-CargoTests $smokeArgs "protected live smoke tests"
    }

    if ($Lane -in @("live", "all")) {
        Invoke-CargoTests $liveArgs "live kernel integration tests"
    }

    if ($Lane -in @("perf", "all")) {
        foreach ($case in $policy.perfCases) {
            $args = @("test", "--locked", "--release", "-p", "frust-kernel", "--test", [string] $case.target)
            $description = "perf $($case.target)"
            if ($case.mode -eq "ignored-exact") {
                $args += @([string] $case.test, "--", "--ignored", "--exact", "--nocapture", "--test-threads=1")
                $description += "::$($case.test)"
            } else {
                $args += @("--", "--nocapture", "--test-threads=1")
            }
            Invoke-CargoTests $args $description
        }
    }
} finally {
    Stop-TestDatabase $database
    [Environment]::SetEnvironmentVariable("CARGO_TARGET_DIR", $savedTargetDir, "Process")
    [Environment]::SetEnvironmentVariable("FRUST_DB_ENDPOINT", $savedEndpoint, "Process")
    [Environment]::SetEnvironmentVariable("FRUST_DB_ROOT_USER", $savedUser, "Process")
    [Environment]::SetEnvironmentVariable("FRUST_DB_ROOT_PASS", $savedPass, "Process")
}

if ($List) {
    Write-Host "`nList mode: no Cargo command or Docker container was started."
} else {
    Write-Host "`nLane '$Lane' passed."
}
