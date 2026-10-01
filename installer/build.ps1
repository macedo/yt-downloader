# Builds the app in release mode and creates the installer in dist\.
# Usage: powershell -ExecutionPolicy Bypass -File installer\build.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Version: $version"

# Rust (rustup) and MinGW (WinLibs via winget), in case this terminal's PATH is stale.
$extra = @("$env:USERPROFILE\.cargo\bin")
$mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\BrechtSanders.WinLibs*\mingw64\bin" -Directory -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($mingw) { $extra += $mingw.FullName }
$env:Path = ($extra -join ';') + ';' + $env:Path

Push-Location $root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
} finally {
    Pop-Location
}

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { $iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source }
if (-not $iscc) { throw 'Inno Setup not found. Install it with: winget install -e --id JRSoftware.InnoSetup' }

& $iscc "/DAppVersion=$version" "$PSScriptRoot\yt-downloader.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC failed ($LASTEXITCODE)" }

Write-Host "Installer created: $root\dist\YT-Downloader-Setup-$version.exe"
