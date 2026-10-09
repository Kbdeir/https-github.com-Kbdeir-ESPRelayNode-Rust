. (Join-Path $PSScriptRoot "common.ps1")
$args = @("+stable", "build", "-p", "node-core", "--example", "preview", "--target", "x86_64-pc-windows-msvc", "--config", "unstable.build-std=[]")
Invoke-InProject "cargo" @args
$port = 8090
while (Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction SilentlyContinue) { $port++ }
$env:SMARTCONFIG_PREVIEW_PORT = $port.ToString()
$binary = Join-Path $env:CARGO_TARGET_DIR "x86_64-pc-windows-msvc\debug\examples\preview.exe"
$output = Join-Path $env:TEMP "smartconfig-preview-$port.log"
$errors = Join-Path $env:TEMP "smartconfig-preview-$port.err.log"
$process = Start-Process -FilePath $binary -WorkingDirectory (Get-ProjectRoot) -WindowStyle Hidden -RedirectStandardOutput $output -RedirectStandardError $errors -PassThru
Write-Host "Preview: http://127.0.0.1:$port/"
Write-Host "Preview-only login: user / preview-local"
Write-Host "Simulated I/O; process ID $($process.Id); logs: $output"
