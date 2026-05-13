# build_pgo.ps1 — Profile-Guided Optimization build for DL-Voice-Typing
#
# Prerequisites:
#   - Vulkan SDK 1.4.341.1+ installed at C:\VulkanSDK\1.4.341.1
#   - Visual Studio 2026 Build Tools with C++ workload
#   - Rust nightly toolchain (rustup toolchain install nightly)
#
# Usage:
#   .\scripts\build_pgo.ps1    # Full PGO build (instrument → train → optimize)

$ErrorActionPreference = "Stop"

# --- Verify nightly toolchain ---
$toolchains = rustup toolchain list 2>$null
if ($LASTEXITCODE -ne 0) {
    Write-Error "rustup not found. Install from https://rustup.rs/"
    exit 1
}
if ($toolchains -notmatch "nightly") {
    Write-Error "Nightly toolchain not installed. Run: rustup toolchain install nightly"
    exit 1
}
Write-Host "Nightly toolchain: available" -ForegroundColor Cyan

# --- Vulkan SDK ---
$VulkanSDK = "C:\VulkanSDK\1.4.341.1"
if (-not (Test-Path $VulkanSDK)) {
    Write-Error "Vulkan SDK not found at $VulkanSDK. Install from https://vulkan.lunarg.com/"
    exit 1
}
$env:VULKAN_SDK = $VulkanSDK
$env:PATH = "$VulkanSDK\Bin;$env:PATH"
Write-Host "Vulkan SDK: $VulkanSDK" -ForegroundColor Cyan

# --- Short target dir (MSVC C1083 workaround) ---
$env:CARGO_TARGET_DIR = "D:\t"
Write-Host "CARGO_TARGET_DIR: $env:CARGO_TARGET_DIR" -ForegroundColor Cyan

# --- VS Developer Environment ---
$vcvars = $null
$vsWhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
if (Test-Path $vsWhere) {
    $vsPath = & $vsWhere -latest -property installationPath 2>$null
    if ($vsPath) {
        $candidate = Join-Path $vsPath "VC\Auxiliary\Build\vcvarsall.bat"
        if (Test-Path $candidate) { $vcvars = $candidate }
    }
}
if (-not $vcvars) {
    $knownPaths = @(
        "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvarsall.bat",
        "C:\Program Files\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvarsall.bat",
        "C:\Program Files (x86)\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvarsall.bat",
        "C:\Program Files\Microsoft Visual Studio\18\Community\VC\Auxiliary\Build\vcvarsall.bat"
    )
    foreach ($p in $knownPaths) {
        if (Test-Path $p) { $vcvars = $p; break }
    }
}
if ($vcvars) {
    Write-Host "Importing VS environment from: $vcvars" -ForegroundColor Cyan
    $output = cmd /c "`"$vcvars`" x64 >nul 2>&1 && set" 2>$null
    foreach ($line in $output) {
        if ($line -match '^([^=]+)=(.*)$') {
            [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
        }
    }
} else {
    Write-Warning "vcvarsall.bat not found. Build may fail if VS environment is not already set."
}

# --- Navigate to src-tauri ---
$projectDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$srcTauri = Join-Path (Split-Path -Parent $projectDir) "src-tauri"
Set-Location $srcTauri

# --- PGO directories ---
$pgoDataDir = "D:\t\pgo-data"
$releaseDir = "D:\t\release"

# ============================================================
# Step 1: Instrumented Build
# ============================================================
Write-Host ""
Write-Host "========================================" -ForegroundColor Yellow
Write-Host "Step 1/5: Instrumented Build" -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Yellow

if (Test-Path $pgoDataDir) {
    Remove-Item -Recurse -Force $pgoDataDir
}
New-Item -ItemType Directory -Path $pgoDataDir -Force | Out-Null

$env:RUSTFLAGS = "-C profile-generate=$pgoDataDir"
Write-Host "RUSTFLAGS: $env:RUSTFLAGS" -ForegroundColor Cyan
Write-Host "Building instrumented binary..." -ForegroundColor Yellow

cargo +nightly tauri build --release -- --no-default-features --features whisper
if ($LASTEXITCODE -ne 0) {
    Write-Error "Instrumented build failed"
    exit 1
}
Write-Host "Instrumented build succeeded." -ForegroundColor Green

# ============================================================
# Step 2: Training Run
# ============================================================
Write-Host ""
Write-Host "========================================" -ForegroundColor Yellow
Write-Host "Step 2/5: Training Run" -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Yellow

# Find the built executable
$exePath = Get-ChildItem -Path "$releaseDir\dl-voice-typing.exe" -ErrorAction SilentlyContinue
if (-not $exePath) {
    $exePath = Get-ChildItem -Path "$releaseDir\bundle\nsis\*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
}
if (-not $exePath) {
    Write-Error "Could not find instrumented binary. Check D:\t\release\"
    exit 1
}

Write-Host ""
Write-Host "Launching instrumented application..." -ForegroundColor Yellow
Write-Host "Please perform the following actions:" -ForegroundColor White
Write-Host "  1. Press and hold RightCtrl, speak 3-5 short phrases, release" -ForegroundColor White
Write-Host "  2. Open the settings page (tray icon -> Settings)" -ForegroundColor White
Write-Host "  3. Switch a few settings (e.g., model, language)" -ForegroundColor White
Write-Host "  4. Close the application completely" -ForegroundColor White
Write-Host ""

Start-Process $exePath.FullName
Read-Host "Press Enter when you have closed the application"

# Verify profile data was generated
$profrawFiles = Get-ChildItem -Path $pgoDataDir -Filter "*.profraw" -ErrorAction SilentlyContinue
if (-not $profrawFiles -or $profrawFiles.Count -eq 0) {
    Write-Warning "No .profraw files found in $pgoDataDir. PGO data may be incomplete."
    Write-Warning "Continuing with optimization build anyway..."
}

# ============================================================
# Step 3: Merge Profile Data
# ============================================================
Write-Host ""
Write-Host "========================================" -ForegroundColor Yellow
Write-Host "Step 3/5: Merge Profile Data" -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Yellow

# Find llvm-profdata from nightly toolchain
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { "$env:USERPROFILE\.rustup" }
$profdataExe = Get-ChildItem -Path "$rustupHome\toolchains\nightly*\lib\rustlib\x86_64-pc-windows-msvc\bin\llvm-profdata.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $profdataExe) {
    Write-Error "llvm-profdata not found in nightly toolchain"
    exit 1
}

$mergedProfdata = Join-Path $pgoDataDir "merged.profdata"
& $profdataExe.FullName merge -o $mergedProfdata (Get-ChildItem -Path $pgoDataDir -Filter "*.profraw").FullName
if ($LASTEXITCODE -ne 0) {
    Write-Error "Profile merge failed"
    exit 1
}
Write-Host "Merged profile data: $mergedProfdata" -ForegroundColor Green

# ============================================================
# Step 4: Optimized Build with Profile Data
# ============================================================
Write-Host ""
Write-Host "========================================" -ForegroundColor Yellow
Write-Host "Step 4/5: Optimized Build" -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Yellow

# Clean previous build artifacts for a fresh PGO build
Remove-Item -Recurse -Force "$releaseDir\build\dl-voice-typing-*" -ErrorAction SilentlyContinue
Remove-Item -Recurse -Force "$releaseDir\bundle\" -ErrorAction SilentlyContinue

$env:RUSTFLAGS = "-C profile-use=$mergedProfdata"
Write-Host "RUSTFLAGS: $env:RUSTFLAGS" -ForegroundColor Cyan
Write-Host "Building optimized binary with PGO data..." -ForegroundColor Yellow

cargo +nightly tauri build --release -- --no-default-features --features whisper
if ($LASTEXITCODE -ne 0) {
    Write-Error "Optimized build failed"
    exit 1
}
Write-Host "PGO-optimized build succeeded." -ForegroundColor Green

# ============================================================
# Step 5: Cleanup
# ============================================================
Write-Host ""
Write-Host "========================================" -ForegroundColor Yellow
Write-Host "Step 5/5: Cleanup" -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Yellow

Remove-Item -Recurse -Force $pgoDataDir
Write-Host "Cleaned up PGO profile data." -ForegroundColor Green

# --- Report ---
Write-Host ""
Write-Host "========================================" -ForegroundColor Green
Write-Host "PGO Build Complete!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Green

$installer = Get-ChildItem -Path "$releaseDir\bundle\nsis\*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if ($installer) {
    Write-Host "Installer: $($installer.FullName)" -ForegroundColor Green
    Write-Host "Size: $([math]::Round($installer.Length / 1MB, 1)) MB" -ForegroundColor Green
} else {
    Write-Host "Output: $releaseDir\" -ForegroundColor Green
}

# Reset RUSTFLAGS
Remove-Item Env:\RUSTFLAGS -ErrorAction SilentlyContinue
