# Bangun MultiTool untuk Windows di komputer Windows (hasil: folder dist\).
# Jalankan dari PowerShell di root repo:   .\packaging\build-windows.ps1
# Syarat sekali pasang: Rust (https://rustup.rs) + "Build Tools for Visual Studio" (workload C++).
# WebView2 sudah ada di Windows 11 / Windows 10 terbaru.
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Rust belum terpasang. Pasang dari https://rustup.rs lalu buka PowerShell baru."
}
if (-not (Get-Command cargo-tauri -ErrorAction SilentlyContinue)) {
    Write-Host "Memasang tauri-cli (sekali saja, beberapa menit)..."
    cargo install tauri-cli --version "^2" --locked
}

Write-Host "Test..."
cargo test -p multitool
if ($LASTEXITCODE -ne 0) { throw "Test gagal" }

Write-Host "Build installer + GUI..."
Push-Location gui
cargo tauri build --bundles nsis
$code = $LASTEXITCODE
Pop-Location
if ($code -ne 0) { throw "Build GUI gagal" }

Write-Host "Build CLI..."
cargo build --release -p multitool
if ($LASTEXITCODE -ne 0) { throw "Build CLI gagal" }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item (Get-ChildItem target\release\bundle\nsis\*-setup.exe | Select-Object -First 1).FullName dist\
Copy-Item target\release\multitool-gui.exe dist\MultiTool-portable.exe
Copy-Item target\release\multitool.exe dist\multitool-cli.exe

Write-Host ""
Write-Host "Selesai. File ada di: $(Resolve-Path dist)"
Get-ChildItem dist | Format-Table Name, Length
