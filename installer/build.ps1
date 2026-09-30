# Compila o app em release e gera o instalador em dist\.
# Uso: powershell -ExecutionPolicy Bypass -File installer\build.ps1
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot

$version = (Select-String -Path "$root\Cargo.toml" -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "Versão: $version"

# Rust (rustup) e MinGW (WinLibs via winget), caso o terminal ainda não tenha o PATH atualizado.
$extra = @("$env:USERPROFILE\.cargo\bin")
$mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\BrechtSanders.WinLibs*\mingw64\bin" -Directory -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($mingw) { $extra += $mingw.FullName }
$env:Path = ($extra -join ';') + ';' + $env:Path

Push-Location $root
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build falhou ($LASTEXITCODE)" }
} finally {
    Pop-Location
}

$iscc = @(
    "$env:LOCALAPPDATA\Programs\Inno Setup 6\ISCC.exe",
    "${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe",
    "$env:ProgramFiles\Inno Setup 6\ISCC.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $iscc) { $iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source }
if (-not $iscc) { throw 'Inno Setup não encontrado. Instale com: winget install -e --id JRSoftware.InnoSetup' }

& $iscc "/DAppVersion=$version" "$PSScriptRoot\yt-downloader.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC falhou ($LASTEXITCODE)" }

Write-Host "Instalador gerado: $root\dist\YT-Downloader-Setup-$version.exe"
