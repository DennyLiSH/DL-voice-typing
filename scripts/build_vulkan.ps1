# build_vulkan.ps1 — Build DL-Voice-Typing with Vulkan GPU acceleration
#
# Prerequisites:
#   - Vulkan SDK 1.4.341.1+ installed at C:\VulkanSDK\1.4.341.1
#   - Visual Studio 2026 Build Tools with C++ workload
#   - Rust 1.85+ (stable-x86_64-pc-windows-msvc)
#
# Usage:
#   .\scripts\build_vulkan.ps1              # Build (debug)
#   .\scripts\build_vulkan.ps1 -Release     # Build release installer
#   .\scripts\build_vulkan.ps1 -Check       # Quick check only
#   .\scripts\build_vulkan.ps1 -Dev         # Run dev mode
#   .\scripts\build_vulkan.ps1 -CleanCache  # Clear WebView2 cache only (no build)

param(
    [switch]$Release,
    [switch]$Check,
    [switch]$Dev,
    [switch]$CleanCache
)

$ErrorActionPreference = "Stop"

# --- Vulkan SDK ---
$VulkanSDK = "C:\VulkanSDK\1.4.341.1"
if (-not (Test-Path $VulkanSDK)) {
    Write-Error "Vulkan SDK not found at $VulkanSDK. Install from https://vulkan.lunarg.com/"
    exit 1
}
$env:VULKAN_SDK = $VulkanSDK
$env:PATH = "$VulkanSDK\Bin;$env:PATH"
Write-Host "Vulkan SDK: $VulkanSDK" -ForegroundColor Cyan

# --- Short target dir (MSVC C1083 workaround for long paths) ---
$env:CARGO_TARGET_DIR = "D:\t"
Write-Host "CARGO_TARGET_DIR: $env:CARGO_TARGET_DIR" -ForegroundColor Cyan

# --- VS Developer Environment ---
# Find vcvarsall.bat — try vswhere first, then known paths
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
    # Known paths for VS 2026 Build Tools
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
    # Import vcvarsall environment into current PowerShell session
    $output = cmd /c "`"$vcvars`" x64 >nul 2>&1 && set" 2>$null
    foreach ($line in $output) {
        if ($line -match '^([^=]+)=(.*)$') {
            [System.Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], 'Process')
        }
    }
} else {
    Write-Warning "vcvarsall.bat not found. Build may fail if VS environment is not already set."
}

# --- WebView2 cache directory ---
# Tauri 2.x stores WebView2 data under %LOCALAPPDATA%\<bundle-identifier>\EBWebView\.
# Only the cache subdirectories are removed; session state (cookies, localStorage, etc.)
# is preserved so the app retains its configuration across cache clears.
$WebView2CacheBase = Join-Path $env:LOCALAPPDATA "xyz.20260304.dl-voice-typing\EBWebView\Default"

# --- CleanCache mode: clear WebView2 cache and exit ---
if ($CleanCache) {
    $cacheDirs = @("Cache", "Code Cache", "Service Worker")
    $removed = 0
    foreach ($dir in $cacheDirs) {
        $path = Join-Path $WebView2CacheBase $dir
        if (Test-Path $path) {
            Remove-Item -Recurse -Force $path
            Write-Host "Removed: $path" -ForegroundColor Yellow
            $removed++
        }
    }
    if ($removed -eq 0) {
        Write-Host "No WebView2 cache directories found (already clean or app never ran)." -ForegroundColor Gray
    } else {
        Write-Host "WebView2 cache cleared ($removed directories)." -ForegroundColor Green
    }
    exit 0
}

# --- Run build ---
$projectDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$srcTauri = Join-Path (Split-Path -Parent $projectDir) "src-tauri"
$projectRoot = Split-Path -Parent $projectDir

Set-Location $srcTauri
Write-Host ""
Write-Host "Working directory: $srcTauri" -ForegroundColor Green

try {
    if ($Check) {
        Write-Host "Running cargo check..." -ForegroundColor Yellow
        cargo check
    } elseif ($Dev) {
        Write-Host "Running cargo tauri dev..." -ForegroundColor Yellow
        cargo tauri dev
    } elseif ($Release) {
        Write-Host "Building release installer (no DevTools)..." -ForegroundColor Yellow
        cargo tauri build -- --no-default-features --features whisper
    } else {
        Write-Host "Running cargo build..." -ForegroundColor Yellow
        cargo build
    }
    $exitCode = $LASTEXITCODE
} finally {
    Set-Location $projectRoot
}

if ($exitCode -eq 0) {
    Write-Host "Build succeeded." -ForegroundColor Green
} else {
    Write-Error "Build failed with exit code $exitCode"
}
exit $exitCode
