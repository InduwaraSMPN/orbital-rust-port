#Requires -Version 5.1
<#
.SYNOPSIS
    Builds the Orbital installer (.exe) using cargo build --release + Inno Setup 6.
.DESCRIPTION
    1. Reads the version from Cargo.toml
    2. Builds the release binary (honours $env:CARGO_TARGET_DIR) to
       <target>\release\Orbital.exe
    3. Runs Inno Setup to produce dist\Orbital-<version>-Setup.exe

    The Rust port has no dotnet publish step: the single natively-compiled
    binary in target\release\ is the whole application, so installer/Orbital.iss
    consumes it directly.
.EXAMPLE
    .\scripts\Build-Installer.ps1
#>

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Root = Split-Path $PSScriptRoot -Parent
Push-Location $Root

try {
    # --- Read version from Cargo.toml ---
    $cargoPath = Join-Path $Root 'Cargo.toml'
    $Version = $null
    foreach ($line in Get-Content $cargoPath) {
        if ($line -match '^\s*version\s*=\s*"([^"]+)"') {
            $Version = $matches[1]
            break
        }
    }
    if (-not $Version) { $Version = '1.0.0' }
    Write-Host "Building installer for Orbital v$Version" -ForegroundColor Cyan

    # --- Resolve cargo ---
    $cargoExe = $env:CARGO
    if (-not $cargoExe) {
        $candidate = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
        if (Test-Path $candidate) { $cargoExe = $candidate }
    }
    if (-not $cargoExe) {
        $found = Get-Command cargo.exe -ErrorAction SilentlyContinue
        if ($found) { $cargoExe = $found.Source }
    }
    if (-not $cargoExe) {
        throw "cargo.exe not found. Install Rust from https://rustup.rs or set `$env:CARGO."
    }

    # --- Resolve cargo's target dir (honours $env:CARGO_TARGET_DIR) ---
    $targetDir = $env:CARGO_TARGET_DIR
    if (-not $targetDir) { $targetDir = Join-Path $Root 'target' }

    # --- cargo build --release ---
    Write-Host "Building release binary..." -ForegroundColor Gray
    & $cargoExe build --release --all-features
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed (exit $LASTEXITCODE)" }

    $releaseDir = Join-Path $targetDir 'release'
    $exePath = Join-Path $releaseDir 'Orbital.exe'
    if (-not (Test-Path $exePath)) {
        throw "Expected $exePath to exist after cargo build --release."
    }

    # --- Run Inno Setup ---
    $iscc = $env:ISCC
    if (-not $iscc) {
        $found = Get-Command iscc.exe -ErrorAction SilentlyContinue
        if ($found) { $iscc = $found.Source }
    }
    if (-not $iscc) {
        $candidates = @(
            'C:\Program Files (x86)\Inno Setup 6\ISCC.exe',
            'C:\Program Files\Inno Setup 6\ISCC.exe',
            'C:\InnoSetupPortable\ISCC.exe'
        )
        foreach ($c in $candidates) {
            if (Test-Path $c) { $iscc = $c; break }
        }
    }
    if (-not $iscc) {
        throw "Inno Setup 6 (ISCC.exe) not found. Install from https://jrsoftware.org/isdl.php or put ISCC.exe on PATH."
    }

    $issPath = Join-Path $Root 'installer\Orbital.iss'
    New-Item -ItemType Directory -Force -Path (Join-Path $Root 'dist') | Out-Null
    Write-Host "Running Inno Setup..." -ForegroundColor Gray
    & $iscc $issPath /DMyAppVersion=$Version "/DReleaseDir=$releaseDir"
    if ($LASTEXITCODE -ne 0) { throw "Inno Setup failed (exit $LASTEXITCODE)" }

    $output = Join-Path $Root "dist\Orbital-$Version-Setup.exe"
    Write-Host ""
    Write-Host "Installer built successfully:" -ForegroundColor Green
    Write-Host "  $output" -ForegroundColor Green
}
finally {
    Pop-Location
}
