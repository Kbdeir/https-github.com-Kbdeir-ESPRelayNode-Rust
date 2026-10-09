param(
    [ValidateSet("debug", "release")]
    [string] $Profile = "release"
)

. (Join-Path $PSScriptRoot "common.ps1")
& (Join-Path $PSScriptRoot "check-tools.ps1")

$args = @("build")
if ($Profile -eq "release") {
    $args += "--release"
}

Invoke-InProject "cargo" @args
$output = Join-Path $env:CARGO_TARGET_DIR 'application-ota.bin'
$imageArgs = @('save-image','--chip','esp32','--partition-table',(Join-Path (Get-ProjectRoot) 'partitions.csv'),'--bootloader',(Get-IdfBootloaderPath -Profile $Profile),'--target-app-partition','ota_0',(Get-FirmwareElfPath -Profile $Profile),$output)
Invoke-InProject 'espflash' @imageArgs
Write-Host "Application-only OTA binary: $output"
