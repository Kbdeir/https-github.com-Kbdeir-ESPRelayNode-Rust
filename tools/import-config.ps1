param([string] $Reference = "C:\Users\kbdeir\Documents\PlatformIO\Projects\SmartConfig - AI", [switch] $EnableNetwork, [switch] $AllowAnyModbus)
. (Join-Path $PSScriptRoot "common.ps1")
$destination = Join-Path (Get-ProjectRoot) "config.local.json"
$args = @("+stable", "run", "-p", "node-core", "--example", "import_config",
    "--target", "x86_64-pc-windows-msvc", "--config", "unstable.build-std=[]",
    "--", $Reference, $destination)
if ($EnableNetwork) { $args += "--enable-network" }
if ($AllowAnyModbus) { $args += "--allow-any-modbus" }
Invoke-InProject "cargo" @args
