$ErrorActionPreference = "Stop"

$secretDirectory = Join-Path $PSScriptRoot "secrets"
New-Item -ItemType Directory -Force -Path $secretDirectory | Out-Null

$usernamePath = Join-Path $secretDirectory "surreal_root_username.txt"
$passwordPath = Join-Path $secretDirectory "surreal_root_password.txt"

if (-not (Test-Path -LiteralPath $usernamePath)) {
    [System.IO.File]::WriteAllText($usernamePath, "root`n")
}

if (-not (Test-Path -LiteralPath $passwordPath)) {
    $bytes = New-Object byte[] 32
    $generator = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try {
        $generator.GetBytes($bytes)
    }
    finally {
        $generator.Dispose()
    }
    $password = ($bytes | ForEach-Object { $_.ToString("x2") }) -join ""
    [System.IO.File]::WriteAllText($passwordPath, "$password`n")
}

Write-Host "Deployment secrets are present in $secretDirectory"
