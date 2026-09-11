# run_cargo_tests.ps1 — Run cargo test/check with full Vulkan + VS env
param(
    [string]$CargoArgs = "check --tests"
)

$ErrorActionPreference = "Stop"

$VulkanSDK = "C:\VulkanSDK\1.4.341.1"
$env:VULKAN_SDK = $VulkanSDK
$env:PATH = "$VulkanSDK\Bin;$env:PATH"
$env:CARGO_TARGET_DIR = "D:\t"

# Hardcoded VS Build Tools path (vswhere unreliable in this shell env).
$vcvars = "C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\VC\Auxiliary\Build\vcvarsall.bat"
if (-not (Test-Path $vcvars)) {
    Write-Error "vcvarsall.bat not found at $vcvars"
    exit 1
}

Set-Location "D:\NC_Work\___DL\voice-typing\src-tauri"
cmd /c "`"$vcvars`" x64 && cargo $CargoArgs"
