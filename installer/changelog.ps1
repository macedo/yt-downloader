# Builds a Release's notes from the commits since the previous tag.
# Usage: installer\changelog.ps1 -Tag v0.5.0 [-Repo macedo/yt-downloader]
param(
    [Parameter(Mandatory)] [string] $Tag,
    [string] $Repo = 'macedo/yt-downloader'
)

# git writes commit messages as UTF-8.
[Console]::OutputEncoding = [Text.Encoding]::UTF8

$previous = git describe --tags --abbrev=0 "$Tag^" 2>$null
$range = if ($previous) { "$previous..$Tag" } else { $Tag }

# Commits whose subject is just the version ("v0.5.0") only bump the number; leave them out.
$changes = @(git log $range --no-merges --reverse --format='- %s (%h)' |
    Where-Object { $_ -notmatch '^- v\d+\.\d+\.\d+ \(' })
if ($LASTEXITCODE -ne 0) { throw "git log failed for $range" }
if ($changes.Count -eq 0) { $changes = @('- No changes besides the version.') }

$version = $Tag.TrimStart('v')
$notes = @('## Changes', '') + $changes
if ($previous) {
    $notes += @('', "**Full changelog:** https://github.com/$Repo/compare/$previous...$Tag")
}
$notes += @(
    '',
    '## Installation',
    '',
    "Download **YT-Downloader-Setup-$version.exe** below and run it. To update, just install over the previous version.",
    '',
    'The installer is not digitally signed: if Windows SmartScreen warns you, click **More info > Run anyway**.'
)
$notes -join "`n"
