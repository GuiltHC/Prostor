# Builds both release variants into .\dist:
#   Prostor.exe       self-contained single file, runs on any Windows 10/11 x64
#   Prostor-lite.exe  framework-dependent single file, needs the .NET 10 Desktop Runtime
# Usage: .\build.ps1 [-Version 1.2.0] [-SkipTests]
# Signing: set PROSTOR_SIGN_PFX (path to .pfx) and PROSTOR_SIGN_PASSWORD before running.
param(
    [string]$Version = "",
    [switch]$SkipTests
)

$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$project = "Prostor\Prostor.csproj"
$dist = Join-Path $PSScriptRoot "dist"
$work = Join-Path $PSScriptRoot "obj\publish"
$common = @("-c", "Release", "-r", "win-x64", "-p:PublishSingleFile=true", "-p:DebugType=none", "-p:GenerateDocumentationFile=false")
if ($Version) { $common += "-p:Version=$Version" }

function Invoke-Dotnet([string[]]$arguments) {
    & dotnet @arguments
    if ($LASTEXITCODE -ne 0) { throw "dotnet $($arguments -join ' ') failed with exit code $LASTEXITCODE" }
}

if (-not $SkipTests) {
    Invoke-Dotnet @("test", "--project", "Prostor.Tests\Prostor.Tests.csproj", "-c", "Release")
}

Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $dist | Out-Null

Invoke-Dotnet (@("publish", $project, "-o", "$work\full", "--self-contained", "true",
    "-p:EnableCompressionInSingleFile=true", "-p:IncludeNativeLibrariesForSelfExtract=true") + $common)
Invoke-Dotnet (@("publish", $project, "-o", "$work\lite", "--self-contained", "false") + $common)

Copy-Item "$work\full\Prostor.exe" "$dist\Prostor.exe" -Force
Copy-Item "$work\lite\Prostor.exe" "$dist\Prostor-lite.exe" -Force
Remove-Item "$dist\*.pdb" -ErrorAction SilentlyContinue

if ($env:PROSTOR_SIGN_PFX) {
    $signtool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\signtool.exe" -ErrorAction SilentlyContinue |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $signtool) { throw "signtool.exe not found (install the Windows SDK)" }
    foreach ($exe in "$dist\Prostor.exe", "$dist\Prostor-lite.exe") {
        & $signtool.FullName sign /fd sha256 /tr http://timestamp.digicert.com /td sha256 /f $env:PROSTOR_SIGN_PFX /p $env:PROSTOR_SIGN_PASSWORD $exe
        if ($LASTEXITCODE -ne 0) { throw "signing $exe failed" }
    }
}

Get-ChildItem "$dist\*.exe" | ForEach-Object { "{0,-18} {1,8:N1} MB" -f $_.Name, ($_.Length / 1MB) }
