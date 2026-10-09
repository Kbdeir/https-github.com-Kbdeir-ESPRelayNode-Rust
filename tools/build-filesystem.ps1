. (Join-Path $PSScriptRoot "common.ps1")
$root = Get-ProjectRoot
$environment = Join-Path $env:CARGO_TARGET_DIR "littlefs-tools"
$tool = Join-Path $environment "Scripts\littlefs-python.exe"
if (-not (Test-Path -LiteralPath $tool)) {
    $python = Get-ChildItem (Join-Path $idfCache "python_env\idf5.3_py*\Scripts\python.exe") -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName
    if (-not $python) { $python = (Get-Command python -ErrorAction Stop).Source }
    & $python -m venv $environment
    if ($LASTEXITCODE -ne 0) { throw "LittleFS tool environment creation failed" }
    & (Join-Path $environment "Scripts\python.exe") -m pip install "littlefs-python==0.19.0"
    if ($LASTEXITCODE -ne 0) { throw "LittleFS image tool installation failed" }
}
$output = Join-Path $env:CARGO_TARGET_DIR "littlefs.bin"
$partition = Get-LittleFSPartition
& $tool create (Join-Path $root "web") $output --block-size=4096 "--fs-size=$($partition.Size)" --name-max=255
if ($LASTEXITCODE -ne 0) { throw "LittleFS image build failed" }
& (Join-Path $environment "Scripts\python.exe") (Join-Path $PSScriptRoot "verify-filesystem.py") $output (Join-Path $root "web") (Join-Path $root 'partitions.csv')
if ($LASTEXITCODE -ne 0) { throw "LittleFS image verification failed" }
Write-Host ("LittleFS image: {0} ({1} KiB, offset 0x{2:x})" -f $output,($partition.Size / 1024),$partition.Offset)
