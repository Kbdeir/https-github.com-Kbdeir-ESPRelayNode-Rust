. (Join-Path $PSScriptRoot "common.ps1")
$args = @("+stable", "test", "--lib", "--tests", "-p", "node-core", "--target", "x86_64-pc-windows-msvc",
    "--config", "unstable.build-std=[]")
Invoke-InProject "cargo" @args
