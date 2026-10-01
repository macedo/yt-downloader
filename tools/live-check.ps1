# Weekly live check against the real YouTube, run on the maintainer's PC
# (YouTube blocks CI runners). Runs the `live_*` tests from src/live_tests.rs
# with the latest yt-dlp and opens/updates a GitHub issue if they fail.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tools\live-check.ps1             # run now
#   powershell -ExecutionPolicy Bypass -File tools\live-check.ps1 -Register   # weekly task
#   powershell -ExecutionPolicy Bypass -File tools\live-check.ps1 -Unregister
#   powershell -ExecutionPolicy Bypass -File tools\live-check.ps1 -NoIssue    # dry run
#
# Needs: Rust (rustup, GNU toolchain + MinGW), Deno, FFmpeg and the GitHub CLI
# logged in (gh auth login). It works in its own clone under
# %LOCALAPPDATA%\yt-downloader-live-check, so it never touches your checkout.
param(
    [switch] $Register,
    [switch] $Unregister,
    # Run the check but never open/comment on a GitHub issue (for trying it out).
    [switch] $NoIssue
)
# 'Continue': git and cargo write progress to stderr, which Windows PowerShell
# would otherwise turn into terminating errors. Exit codes are checked instead.
$ErrorActionPreference = 'Continue'

$Repo = 'macedo/yt-downloader'
$TaskName = 'YT Downloader live check'
$Work = Join-Path $env:LOCALAPPDATA 'yt-downloader-live-check'
$Clone = Join-Path $Work 'repo'
$LogDir = Join-Path $Work 'logs'
New-Item -ItemType Directory -Force $Work, $LogDir | Out-Null

# Fresh PATH from the registry (winget installs land there), plus Rust and MinGW.
$env:Path = @(
    [Environment]::GetEnvironmentVariable('Path', 'Machine'),
    [Environment]::GetEnvironmentVariable('Path', 'User'),
    "$env:USERPROFILE\.cargo\bin"
) -join ';'
$mingw = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\BrechtSanders.WinLibs*\mingw64\bin" -Directory -ErrorAction SilentlyContinue |
    Select-Object -First 1
if ($mingw) { $env:Path = "$($mingw.FullName);$env:Path" }

function Update-Clone {
    if (-not (Test-Path (Join-Path $Clone '.git'))) {
        git clone --quiet "https://github.com/$Repo.git" $Clone
        if ($LASTEXITCODE -ne 0) { throw "git clone failed" }
    }
    # This clone is only used by this script, so resetting it is safe.
    git -C $Clone fetch --quiet --prune origin
    if ($LASTEXITCODE -ne 0) { throw "git fetch failed" }
    git -C $Clone reset --quiet --hard origin/main
}

if ($Unregister) {
    Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
    Write-Host "Removed the scheduled task '$TaskName'."
    exit 0
}

if ($Register) {
    Update-Clone
    # The task runs the copy of this script in the clone, which is reset to
    # origin/main on every run, so it always uses the latest version.
    $script = Join-Path $Clone 'tools\live-check.ps1'
    $action = New-ScheduledTaskAction -Execute 'powershell.exe' `
        -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -File `"$script`""
    $trigger = New-ScheduledTaskTrigger -Weekly -DaysOfWeek Monday -At 10am
    # Runs later if the PC was off on Monday morning; needs a network.
    $settings = New-ScheduledTaskSettingsSet -StartWhenAvailable -RunOnlyIfNetworkAvailable `
        -ExecutionTimeLimit (New-TimeSpan -Hours 1)
    Register-ScheduledTask -TaskName $TaskName -Action $action -Trigger $trigger -Settings $settings `
        -Description "Weekly check of github.com/$Repo against the real YouTube (tools/live-check.ps1)." -Force | Out-Null
    Write-Host "Registered '$TaskName': Mondays at 10:00 (or as soon as possible after)."
    exit 0
}

$log = Join-Path $LogDir ("live-check-{0:yyyy-MM-dd-HHmm}.log" -f (Get-Date))
Start-Transcript -Path $log | Out-Null
try {
    Update-Clone
    Write-Host "Checking $(git -C $Clone log --oneline -1)"

    # The latest yt-dlp, only for this check (your own yt-dlp is left alone).
    $bin = Join-Path $Work 'bin'
    New-Item -ItemType Directory -Force $bin | Out-Null
    $env:YTDLP = Join-Path $bin 'yt-dlp.exe'
    Invoke-WebRequest https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe -OutFile $env:YTDLP
    Write-Host "yt-dlp $(& $env:YTDLP --version)"

    Push-Location $Clone
    try {
        $output = cargo test --release --locked live_ -- --ignored --nocapture 2>&1 | ForEach-Object { "$_" }
        $exit = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $output | Write-Host
} catch {
    # Setup failures (git, download...) also count as a failed check.
    $output = @("live check failed: $_")
    $exit = 1
    $output | Write-Host
} finally {
    Stop-Transcript | Out-Null
}

# Keep the last 20 logs.
Get-ChildItem $LogDir -Filter 'live-check-*.log' | Sort-Object Name -Descending |
    Select-Object -Skip 20 | Remove-Item

if ($exit -ne 0 -and -not $NoIssue) {
    # Only the test results go into the (public) issue, without local paths.
    $details = ($output | Select-String -Pattern 'panicked|estimates|nothing was checked|failed|test live_' |
        ForEach-Object { $_.Line.Replace($env:USERPROFILE, '~') }) -join "`n"
    $body = "The weekly live check against YouTube failed on the maintainer's PC ($(Get-Date -Format yyyy-MM-dd)).`n`n" +
        "yt-dlp or YouTube may have changed in a way that affects the link preview's size estimate.`n`n" +
        "``````text`n$details`n```````n`nFull log on the PC: ``$($log.Replace($env:USERPROFILE, "~"))``"
    gh label create upstream-check --repo $Repo --color B60205 --description "Weekly yt-dlp/YouTube check failed" --force | Out-Null
    $existing = gh issue list --repo $Repo --label upstream-check --state open --json number --jq '.[0].number'
    if ($existing) {
        gh issue comment $existing --repo $Repo --body $body
    } else {
        gh issue create --repo $Repo --title "Live check against YouTube failed" --label upstream-check --body $body
    }
}
exit $exit
