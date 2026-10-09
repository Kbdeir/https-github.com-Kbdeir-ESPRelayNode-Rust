. (Join-Path $PSScriptRoot "common.ps1")

Import-EspEnvironment

Assert-Command "cargo" "Install Rust with rustup, then run .\setup-tools.cmd from this repository."
Assert-Command "rustup" "Install Rust with rustup, then run .\setup-tools.cmd from this repository."
Assert-Command "ldproxy" "Run: cargo install ldproxy --locked"
Assert-Command "espflash" "Run: cargo install espflash --locked"

$toolchains = (& rustup toolchain list) -join "`n"
if ($toolchains -notmatch "(^|\s)esp(\s|$)") {
    throw "Rust ESP toolchain 'esp' is not installed. Run: cargo install espup --locked; espup install"
}
