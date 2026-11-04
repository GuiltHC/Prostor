# Builds the release exe into .\dist:
#   Prostor.exe       the program (one file, nothing to install)
#   Prostor-lite.exe  the same file under the name the old .NET "lite" build updates itself from,
#                     so users of either 1.x build move to 2.x through the built-in updater
# Usage: .\build.ps1 [-SkipTests]
# Signing: set PROSTOR_SIGN_PFX (path to .pfx) and PROSTOR_SIGN_PASSWORD before running.
param([switch]$SkipTests)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

function Invoke-Cargo([string[]]$arguments) {
    & cargo @arguments
    if ($LASTEXITCODE -ne 0) { throw "cargo $($arguments -join ' ') failed with exit code $LASTEXITCODE" }
}

if (-not $SkipTests) {
    Invoke-Cargo @("test")
}
Invoke-Cargo @("build", "--release")

$dist = Join-Path $PSScriptRoot "dist"
New-Item -ItemType Directory -Force $dist | Out-Null
Copy-Item "target\release\prostor.exe" "$dist\Prostor.exe" -Force

if ($env:PROSTOR_SIGN_PFX) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $signtool) { throw "signtool.exe not found (install the Windows SDK)" }
    & $signtool.FullName sign /fd sha256 /tr http://timestamp.digicert.com /td sha256 /f $env:PROSTOR_SIGN_PFX /p $env:PROSTOR_SIGN_PASSWORD "$dist\Prostor.exe"
    if ($LASTEXITCODE -ne 0) { throw "signing failed" }
}

Copy-Item "$dist\Prostor.exe" "$dist\Prostor-lite.exe" -Force
Get-ChildItem "$dist\*.exe" | ForEach-Object { "{0,-18} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB) }
