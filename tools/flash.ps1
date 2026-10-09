param(
    [ValidateSet("debug", "release")]
    [string] $Profile = "release",
    [string] $Port = "COM3",
    [int] $Baud = 460800,
    [int] $MonitorBaud = 115200,
    [switch] $ManualBoot,
    [switch] $NoMonitor,
    [switch] $InitializeOta
)

. (Join-Path $PSScriptRoot "common.ps1")
& (Join-Path $PSScriptRoot "check-tools.ps1")

& (Join-Path $PSScriptRoot "build.ps1") -Profile $Profile

$elf = Get-FirmwareElfPath -Profile $Profile
if (-not (Test-Path -LiteralPath $elf)) {
    throw "Expected firmware ELF was not produced: $elf"
}

$args = @("flash", "--chip", "esp32", "--bootloader", (Get-IdfBootloaderPath -Profile $Profile), "--partition-table", (Join-Path (Get-ProjectRoot) "partitions.csv"), "--target-app-partition", "ota_0", "--erase-parts", $(if ($InitializeOta) {'otadata,ota_1'} else {'otadata'}), "--baud", $Baud.ToString())
Write-Host 'Serial install selects ota_0 by resetting OTA metadata. NVS is not erased.'
if ($InitializeOta) { Write-Host 'Initial OTA migration: ota_1 is also erased; flash the matching LittleFS image before RESET.' }
if ($ManualBoot) {
    $args += @("--before", "no-reset", "--after", "no-reset")
    Write-Host "Manual boot mode: enter download mode with BOOT/GPIO0 held during RESET before flashing."
}
if ($Port) {
    $args += @("--port", $Port)
}
$args += $elf

Invoke-InProject "espflash" @args
Write-Host "Flash complete. Press the board's RESET button to start the firmware."
if (-not $NoMonitor) {
    & (Join-Path $PSScriptRoot "monitor.ps1") -Port $Port -Baud $MonitorBaud
}
