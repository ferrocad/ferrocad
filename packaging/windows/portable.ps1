# Build a portable FerroCAD distribution (and a .zip) for Windows.
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\portable.ps1 [-DebugBuild]
#       [-Edition parametric] [-Occt] [-CheckInit]
#
# The base artifact is ferrocad-windows-x86_64.zip; an edition's is
# ferrocad-<edition>-windows-x86_64.zip (see
# docs/architecture/edition-publishing.md). `-Occt` copies the OCCT DLL closure
# into bin/ beside ferrocad.exe, where the loader finds it first.
#
# Requires a Rust toolchain and CPython headers (PyO3). An MSIX/WiX installer is a
# later step (see docs/architecture/distribution.md).
[CmdletBinding()]
param(
    # Named `DebugBuild`, not `Debug`: `[CmdletBinding()]` already contributes a
    # common `-Debug` parameter, and a second one is a hard error.
    [switch]$DebugBuild,
    [string]$Edition = '',
    [switch]$Occt,
    [switch]$CheckInit
)
$ErrorActionPreference = 'Stop'

# `bundle` spells the edition `parametric`; accept the crate name too. `base` and
# `ferrocad` mean the base app, whose artifact has no edition segment.
if ($Edition -eq 'ferrocad_parametric' -or $Edition -eq 'ferrocad-parametric') {
    $Edition = 'parametric'
}
if ($Edition -eq 'ferrocad' -or $Edition -eq 'base') {
    $Edition = ''
}

$Here = Split-Path -Parent $MyInvocation.MyCommand.Path
$Root = (Resolve-Path (Join-Path $Here '..\..')).Path
Push-Location $Root
try {
    $bundleArgs = @()
    if ($DebugBuild) { $bundleArgs += '--debug' }
    if ($Edition) { $bundleArgs += @('--edition', $Edition) }
    if ($Occt) { $bundleArgs += '--occt' }
    if ($CheckInit) { $bundleArgs += '--check-init' }
    & cargo xtask bundle @bundleArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo xtask bundle failed" }

    $PayloadName = if ($Edition) { "ferrocad-$Edition" } else { 'ferrocad' }
    $ArtifactName = if ($Edition) { "ferrocad-$Edition-windows-x86_64" } else { 'ferrocad-windows-x86_64' }
    $Payload = Join-Path $Root "target\dist\$PayloadName"
    $Out = Join-Path $Root "target\dist\$ArtifactName"
    if (-not (Test-Path $Payload)) { throw "no staged payload at $Payload" }
    if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
    New-Item -ItemType Directory -Path $Out -Force | Out-Null
    Copy-Item -Recurse -Force (Join-Path $Payload '*') $Out

    # A convenience launcher that points the embedded interpreter at the payload
    # and, when bundled, at the Python runtime. The OCCT DLLs (and anything else
    # `bundle --occt` staged) sit in bin/ beside the .exe; adding both bin/ and
    # lib/ to PATH also covers a payload assembled with DLLs under lib/.
    $bat = @'
@echo off
set "HERE=%~dp0"
if exist "%HERE%runtime" (
  set "PYTHONHOME=%HERE%runtime"
  set "PATH=%HERE%runtime;%HERE%runtime\Scripts;%PATH%"
)
if exist "%HERE%lib" set "PATH=%HERE%lib;%PATH%"
set "PATH=%HERE%bin;%PATH%"
set "FERROCAD_PYTHON_PATH=%HERE%python"
set "FERROCAD_MODS_PATH=%HERE%mods"
set "PYTHONPATH=%HERE%python;%HERE%mods;%PYTHONPATH%"
"%HERE%bin\ferrocad.exe" %*
'@
    Set-Content -Encoding ASCII -Path (Join-Path $Out 'ferrocad.bat') -Value $bat

    $Zip = Join-Path $Root "target\dist\$ArtifactName.zip"
    if (Test-Path $Zip) { Remove-Item -Force $Zip }
    Compress-Archive -Path (Join-Path $Out '*') -DestinationPath $Zip

    Write-Host "created $Out"
    Write-Host "created $Zip"
}
finally {
    Pop-Location
}
