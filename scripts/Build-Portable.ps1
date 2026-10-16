#Requires -Version 5.1
<#
.SYNOPSIS
    Builds the Orbital portable distribution (.zip) using cargo build --release.
.DESCRIPTION
    1. Reads the version from Cargo.toml
    2. Builds the release binary (honours $env:CARGO_TARGET_DIR) to
       <target>\release\Orbital.exe
    3. Bundles the binary + Cleanup.cmd into dist\Orbital-<version>-Portable.zip

    The Rust release binary is self-contained (natively linked, no .NET runtime),
    so the portable zip is just the exe plus the settings-cleanup helper.
.EXAMPLE
    .\scripts\Build-Portable.ps1
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
    Write-Host "Building portable package for Orbital v$Version" -ForegroundColor Cyan

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

    $exePath = Join-Path $targetDir 'release\Orbital.exe'
    if (-not (Test-Path $exePath)) {
        throw "Expected $exePath to exist after cargo build --release."
    }

    # --- Build zip ---
    $folderName  = "Orbital-$Version"
    $stagingDir  = Join-Path $targetDir "staging\$folderName"
    if (Test-Path $stagingDir) { Remove-Item $stagingDir -Recurse -Force }
    New-Item -ItemType Directory -Force -Path $stagingDir | Out-Null

    # Copy the release binary and the settings-cleanup helper.
    Copy-Item $exePath $stagingDir
    $cleanupSrc = Join-Path $Root 'portable\Cleanup.cmd'
    if (Test-Path $cleanupSrc) {
        Copy-Item $cleanupSrc $stagingDir
    }

    # Create zip
    New-Item -ItemType Directory -Force -Path (Join-Path $Root 'dist') | Out-Null
    $zipPath = Join-Path $Root "dist\Orbital-$Version-Portable.zip"
    if (Test-Path $zipPath) { Remove-Item $zipPath -Force }

    Compress-Archive -Path $stagingDir -DestinationPath $zipPath

    Write-Host ""
    Write-Host "Portable package built successfully:" -ForegroundColor Green
    Write-Host "  $zipPath" -ForegroundColor Green

    # Cleanup staging
    Remove-Item (Join-Path $Root 'target\staging') -Recurse -Force -ErrorAction SilentlyContinue
}
finally {
    Pop-Location
}
