param([string] $Port = 'COM3', [int] $Baud = 115200, [ValidateRange(1,3600)][int] $Seconds = 300)
$ErrorActionPreference = 'Stop'
$serial = [System.IO.Ports.SerialPort]::new($Port,$Baud,[System.IO.Ports.Parity]::None,8,[System.IO.Ports.StopBits]::One)
$serial.DtrEnable = $false
$serial.RtsEnable = $false
$logPath = Join-Path $env:TEMP ('smartconfig-serial-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.log')
$writer = [System.IO.StreamWriter]::new($logPath,$false)
$writer.AutoFlush = $true
try {
    $serial.Open()
    Write-Output "Serial capture open: $logPath"
    $until = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $until) {
        $chunk = $serial.ReadExisting()
        if ($chunk) { $writer.Write($chunk); Write-Output $chunk }
        Start-Sleep -Milliseconds 100
    }
} finally {
    $serial.Close()
    $serial.Dispose()
    $writer.Dispose()
}
