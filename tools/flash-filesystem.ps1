param([string] $Port = "COM3", [int] $Baud = 115200, [switch] $ManualBoot)
. (Join-Path $PSScriptRoot "common.ps1")
& (Join-Path $PSScriptRoot "check-tools.ps1")
& (Join-Path $PSScriptRoot "build-filesystem.ps1")
Write-Host "Replacing LittleFS web files only; NVS settings are not erased."
$args = @("write-bin", "--chip", "esp32", "--port", $Port, "--baud", $Baud.ToString())
if ($ManualBoot) { $args += @("--before", "no-reset", "--after", "no-reset") }
$partition = Get-LittleFSPartition
$args += @(("0x{0:x}" -f $partition.Offset), (Join-Path $env:CARGO_TARGET_DIR "littlefs.bin"))
Invoke-InProject "espflash" @args
Write-Host "Filesystem flash complete. With BOOT released, press RESET."
