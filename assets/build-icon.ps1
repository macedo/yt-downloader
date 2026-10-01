# Regenerates assets\icon.ico and assets\icon-256.png from the SVG sources.
# Run it after editing icon.svg or icon-small.svg, and commit the results.
#
# Needs resvg (an SVG renderer): cargo install resvg --locked
# Usage: powershell -ExecutionPolicy Bypass -File assets\build-icon.ps1
$ErrorActionPreference = 'Stop'
$assets = $PSScriptRoot

$resvg = (Get-Command resvg -ErrorAction SilentlyContinue).Source
if (-not $resvg) { $resvg = "$env:USERPROFILE\.cargo\bin\resvg.exe" }
if (-not (Test-Path $resvg)) { throw 'resvg not found. Install it with: cargo install resvg --locked' }

# Small sizes use the simplified drawing, which stays crisp at 16-24 px.
$sizes = @(
    @{ Size = 16;  Svg = 'icon-small.svg' },
    @{ Size = 20;  Svg = 'icon-small.svg' },
    @{ Size = 24;  Svg = 'icon-small.svg' },
    @{ Size = 32;  Svg = 'icon.svg' },
    @{ Size = 40;  Svg = 'icon.svg' },
    @{ Size = 48;  Svg = 'icon.svg' },
    @{ Size = 64;  Svg = 'icon.svg' },
    @{ Size = 128; Svg = 'icon.svg' },
    @{ Size = 256; Svg = 'icon.svg' }
)

$tmp = Join-Path ([IO.Path]::GetTempPath()) 'yt-downloader-icon'
New-Item -ItemType Directory -Force $tmp | Out-Null
$images = foreach ($s in $sizes) {
    $png = Join-Path $tmp "icon-$($s.Size).png"
    & $resvg -w $s.Size -h $s.Size (Join-Path $assets $s.Svg) $png
    if ($LASTEXITCODE -ne 0) { throw "resvg failed for $($s.Size) px" }
    @{ Size = $s.Size; Bytes = [IO.File]::ReadAllBytes($png) }
}

# ICO file: a 6-byte header, a 16-byte entry per image, then the images
# (PNG-compressed, supported by Windows Vista and later).
$out = New-Object IO.MemoryStream
$w = New-Object IO.BinaryWriter($out)
$w.Write([uint16]0)               # reserved
$w.Write([uint16]1)               # type: icon
$w.Write([uint16]$images.Count)
$offset = 6 + 16 * $images.Count
foreach ($img in $images) {
    $dim = if ($img.Size -ge 256) { 0 } else { $img.Size }  # 0 means 256
    $w.Write([byte]$dim)          # width
    $w.Write([byte]$dim)          # height
    $w.Write([byte]0)             # palette colors
    $w.Write([byte]0)             # reserved
    $w.Write([uint16]1)           # color planes
    $w.Write([uint16]32)          # bits per pixel
    $w.Write([uint32]$img.Bytes.Length)
    $w.Write([uint32]$offset)
    $offset += $img.Bytes.Length
}
foreach ($img in $images) { $w.Write($img.Bytes) }
$w.Flush()
[IO.File]::WriteAllBytes((Join-Path $assets 'icon.ico'), $out.ToArray())

# The window icon (loaded by the app at runtime).
Copy-Item (Join-Path $tmp 'icon-256.png') (Join-Path $assets 'icon-256.png') -Force
Remove-Item -Recurse -Force $tmp

Write-Host "Wrote icon.ico ($($images.Count) sizes) and icon-256.png"
