$ErrorActionPreference = "Stop"

$Name = "herdr-jumplist"
$Root = Split-Path -Parent $PSScriptRoot

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "$Name : cargo not found; install a Rust toolchain (https://rustup.rs)"
}

cargo build --release --manifest-path "$Root\Cargo.toml"
if ($LASTEXITCODE -ne 0) {
    Write-Host "$Name : build failed. If cargo reported an unsupported rustc version, run 'rustup update' and reinstall."
    exit $LASTEXITCODE
}

New-Item -ItemType Directory -Force -Path "$Root\bin" | Out-Null
Copy-Item -Force "$Root\target\release\$Name.exe" "$Root\bin\$Name.exe"
Write-Host "$Name : installed $Root\bin\$Name.exe"
