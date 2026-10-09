$ErrorActionPreference = "Stop"

function Invoke-Native {
    param(
        [Parameter(Mandatory = $true)]
        [string] $Exe,
        [Parameter(ValueFromRemainingArguments = $true)]
        [string[]] $Args
    )

    & $Exe @Args
    if ($LASTEXITCODE -ne 0) {
        throw "$Exe $($Args -join ' ') failed with exit code $LASTEXITCODE"
    }
}

$cargoBin = Join-Path $HOME ".cargo\bin"
if ((Test-Path -LiteralPath $cargoBin) -and (($env:Path -split ";") -notcontains $cargoBin)) {
    $env:Path = "$cargoBin;$env:Path"
}

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "cargo was not found on PATH. Install Rust with rustup first: https://rustup.rs/"
}

Invoke-Native cargo +stable install espup --locked
Invoke-Native espup install

$exportScript = Join-Path $HOME "export-esp.ps1"
if (Test-Path -LiteralPath $exportScript) {
    . $exportScript
}

Invoke-Native cargo +stable install ldproxy --locked
Invoke-Native cargo +stable install espflash --locked

Write-Host "ESP Rust tools are installed. Restart your shell, or run: . `$HOME\export-esp.ps1"
