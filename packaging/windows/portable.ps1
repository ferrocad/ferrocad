# Build a portable FerroCAD distribution (and a .zip) for Windows.
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\portable.ps1 [-Debug]
#
# Requires a Rust toolchain and CPython headers (PyO3). An MSIX/WiX installer is a
# later step (see docs/distribution.md).
[CmdletBinding()]
param(
    [switch]$Debug
)
$ErrorActionPreference = 'Stop'

$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = (Resolve-Path (Join-Path $Here '..\..')).Path
Push-Location $Root
try {
    $bundleArgs = @()
    if ($Debug) { $bundleArgs += '--debug' }
    & cargo xtask bundle @bundleArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo xtask bundle failed" }

    $Payload = Join-Path $Root 'target\dist\ferrocad'
    $Out = Join-Path $Root 'target\dist\ferrocad-windows-x86_64'
    if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
    New-Item -ItemType Directory -Path $Out -Force | Out-Null
    Copy-Item -Recurse -Force (Join-Path $Payload '*') $Out

    # A convenience launcher that points the embedded interpreter at the payload.
    $bat = @'
@echo off
set "HERE=%~dp0"
set "FERROCAD_PYTHON_PATH=%HERE%python"
set "PYTHONPATH=%HERE%lib;%HERE%python;%HERE%mods;%PYTHONPATH%"
"%HERE%bin\ferrocad.exe" %*
'@
    Set-Content -Encoding ASCII -Path (Join-Path $Out 'ferrocad.bat') -Value $bat

    $Zip = Join-Path $Root 'target\dist\ferrocad-windows-x86_64.zip'
    if (Test-Path $Zip) { Remove-Item -Force $Zip }
    Compress-Archive -Path (Join-Path $Out '*') -DestinationPath $Zip

    Write-Host "created $Out"
    Write-Host "created $Zip"
}
finally {
    Pop-Location
}
