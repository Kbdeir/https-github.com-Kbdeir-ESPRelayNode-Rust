param(
    [string] $Port = "COM3",
    [int] $Baud = 115200
)

. (Join-Path $PSScriptRoot "common.ps1")

$serial = [System.IO.Ports.SerialPort]::new(
    $Port, $Baud, [System.IO.Ports.Parity]::None, 8, [System.IO.Ports.StopBits]::One)
try {
    $serial.Open()
    Write-Host "Listening on $Port at $Baud baud. Press RESET for startup logs; Ctrl+C closes the monitor."
    while ($true) {
        $output = $serial.ReadExisting()
        if ($output.Length -gt 0) { [Console]::Write($output) }
        Start-Sleep -Milliseconds 50
    }
}
finally {
    if ($serial.IsOpen) { $serial.Close() }
    $serial.Dispose()
}
