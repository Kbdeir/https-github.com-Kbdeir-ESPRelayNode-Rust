$ErrorActionPreference = "Stop"

$cargoBin = Join-Path $HOME ".cargo\bin"
if ((Test-Path -LiteralPath $cargoBin) -and (($env:Path -split ";") -notcontains $cargoBin)) {
    $env:Path = "$cargoBin;$env:Path"
}

if (-not $env:CARGO_TARGET_DIR) {
    $env:CARGO_TARGET_DIR = "C:\esp\rn"
}
New-Item -ItemType Directory -Force -Path $env:CARGO_TARGET_DIR | Out-Null

# Reuse the short-path IDF cache while metadata resolves from the actual workspace.
$idfCache = Join-Path (Split-Path $env:CARGO_TARGET_DIR -Parent) ".embuild\espressif"
if (-not $env:ESP_IDF_TOOLS_INSTALL_DIR -and (Test-Path -LiteralPath $idfCache)) {
    $env:ESP_IDF_TOOLS_INSTALL_DIR = "custom:$idfCache"
}

function Get-ProjectRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
}

function Import-EspEnvironment {
    $exportScript = Join-Path $HOME "export-esp.ps1"
    if (Test-Path -LiteralPath $exportScript) {
        . $exportScript
    }
}

function Assert-Command {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Name,
        [Parameter(Mandatory = $true)]
        [string] $InstallHint
    )

    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "$Name was not found on PATH. $InstallHint"
    }
}

function Invoke-InProject {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Command,
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]] $CommandArgs
    )

    $root = Get-ProjectRoot
    Push-Location $root
    try {
        & $Command @CommandArgs
        if ($LASTEXITCODE -ne 0) {
            throw "$Command $($CommandArgs -join ' ') failed with exit code $LASTEXITCODE"
        }
    }
    finally {
        Pop-Location
    }
}

function Get-FirmwareElfPath {
    param(
        [Parameter(Mandatory = $true)]
        [ValidateSet("debug", "release")]
        [string] $Profile
    )

    $leaf = if ($Profile -eq "release") { "release" } else { "debug" }
    return Join-Path $env:CARGO_TARGET_DIR "xtensa-esp32-espidf\$leaf\esp-relay-node-rust"
}

function Get-LittleFSPartition {
    $partition = Get-Content (Join-Path (Get-ProjectRoot) 'partitions.csv') |
        Where-Object { $_.Trim() -and -not $_.Trim().StartsWith('#') } |
        ConvertFrom-Csv -Header Name,Type,Subtype,Offset,Size,Flags |
        Where-Object { $_.Name.Trim() -eq 'littlefs' }
    if (@($partition).Count -ne 1) { throw 'Expected exactly one LittleFS partition' }
    return [pscustomobject]@{ Offset = [Convert]::ToInt32($partition.Offset.Trim(),16); Size = [Convert]::ToInt32($partition.Size.Trim(),16) }
}

function Get-IdfBootloaderPath {
    param([ValidateSet('debug','release')][string] $Profile = 'release')
    $files = Get-ChildItem (Join-Path $env:CARGO_TARGET_DIR "xtensa-esp32-espidf\$Profile\build\esp-idf-sys-*\out\build\bootloader\bootloader.bin")
    $bootloader = $files | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $bootloader) { throw 'Matching ESP-IDF bootloader was not built' }
    $config = Join-Path $bootloader.DirectoryName '..\..\sdkconfig'
    if (-not (Select-String -LiteralPath $config -Pattern '^CONFIG_BOOTLOADER_APP_ROLLBACK_ENABLE=y$' -Quiet)) { throw 'Matching bootloader must have OTA rollback enabled; rebuild first' }
    return $bootloader.FullName
}
