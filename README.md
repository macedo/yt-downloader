# YT Downloader

A Windows app for downloading video and audio from YouTube (and other sites supported by [yt-dlp](https://github.com/yt-dlp/yt-dlp)), with a native GUI written in Rust ([egui](https://github.com/emilk/egui)).

The app is a front end for `yt-dlp`: it builds the arguments, tracks progress and shows the result. Downloads and conversions are done by yt-dlp and FFmpeg.

## Features

- **Video (MP4)** in qualities from "Best available" down to 360p
- **Audio only** as MP3, M4A, Opus, FLAC, WAV or the original format, with a choice of bitrate
- **Link preview**: when you paste a link, shows the thumbnail, title, channel, duration, chapters and approximate file size
- **Several links** at once (one per line) and **whole playlists**
- **Split by chapters**: one file per chapter, handy for full albums
- **Download only a clip**, e.g. from `1:30` to `2:45`
- **Cover art and metadata** embedded in the file
- **New yt-dlp version notice** when the app opens, with one-click update
- Light, dark or system theme; settings are remembered between sessions
- Shortcuts: **Ctrl+Enter** downloads, **Esc** cancels

## Installation

Download `YT-Downloader-Setup-<version>.exe` from the [Releases](https://github.com/macedo/yt-downloader/releases) page and run it:

- No admin rights needed; it installs for the current user in `%LOCALAPPDATA%\Programs\YT Downloader`.
- By default it installs or updates the dependencies through **winget**: yt-dlp, FFmpeg and Deno. YouTube currently requires Deno.
- To update the app, run the newer installer over the current install.

The installer and the app are **not digitally signed**. Because of that:

- Windows SmartScreen may show an "unknown publisher" warning. Click **More info → Run anyway**.
- On PCs with **Smart App Control** turned on, the app is blocked.

### Requirements

- Windows 10 or 11 (64-bit)
- [winget](https://learn.microsoft.com/windows/package-manager/winget/), included with Windows 11, to install the dependencies
- An internet connection

If yt-dlp is not installed, the app shows an **Install yt-dlp** button in the toolbar.

## Usage

1. Paste one or more links into the **Links** field.
2. Choose **Video** or **Audio** in the toolbar.
3. Adjust the format, quality and options in the left panel.
4. Click **Download** or press Ctrl+Enter.

Files are saved to the **Destination** folder, which defaults to your Downloads folder. Settings are stored in `%APPDATA%\yt-downloader\config.json`.

### Notes

- **Estimated size**: follows the same format choice as yt-dlp. For some videos YouTube doesn't report the size of the best quality ("Premium" formats); in that case the app shows "Size not reported". For MP3 VBR and FLAC the value is approximate.
- **Split by chapters**: yt-dlp also keeps the full file. Each chapter file gets the right name, but the title stored in its metadata is the video's title.
- **Clips**: the clip is re-encoded so the cut is exact. For 4K videos this can take a while.

## Development

### Setup

The project uses Rust's **GNU** toolchain, which doesn't need Visual Studio:

```bash
winget install -e --id Rustlang.Rustup
winget install -e --id BrechtSanders.WinLibs.POSIX.UCRT
winget install -e --id yt-dlp.yt-dlp
```

```bash
rustup default stable-x86_64-pc-windows-gnu
```

MinGW (WinLibs) must be on the `PATH` while building, because the `dlltool` bundled with rustup doesn't work on its own. winget adds MinGW to the PATH; open a new terminal after installing it.

> With **Smart App Control** turned on, Windows blocks the executables produced by `cargo`, including build scripts, and the build fails.

### Build and test

```bash
cargo build --release
```

```bash
cargo test --release
```

The executable is written to `target\release\yt-downloader.exe`.

### Build the installer

The installer is built with [Inno Setup](https://jrsoftware.org/isinfo.php):

```bash
winget install -e --id JRSoftware.InnoSetup
```

```bash
powershell -ExecutionPolicy Bypass -File installer\build.ps1
```

The script builds the app in release mode and creates `dist\YT-Downloader-Setup-<version>.exe`, using the version from `Cargo.toml`.

### Layout

| Path | Contents |
|---|---|
| `src/main.rs` | Entry point: window options and module list |
| `src/app.rs` | App state and the egui UI (toolbar, options panel, links, log, status bar) |
| `src/args.rs` | Builds the yt-dlp download arguments from the settings |
| `src/settings.rs` | User settings, quality/format lists, saved to `%APPDATA%` |
| `src/media.rs` | Link preview: parses `yt-dlp -J` output and estimates the file size |
| `src/process.rs` | Runs yt-dlp/winget without a console window and finds yt-dlp on the PATH |
| `src/update.rs` | Checks GitHub for a newer yt-dlp version |
| `src/clip.rs` | Parses clip times (`1:30`) and formats them for file names |
| `src/net.rs` | Shared HTTP client |
| `src/live_tests.rs` | Live checks against the real yt-dlp and YouTube (ignored by default) |
| `installer/yt-downloader.iss` | Inno Setup script (install, shortcuts, dependencies via winget, uninstall) |
| `installer/build.ps1` | Builds the app and the installer |
| `installer/changelog.ps1` | Builds a Release's notes from the commits since the previous tag |
| `.github/workflows/ci.yml` | Checks every pull request and push to `main` |
| `.github/workflows/release.yml` | Builds and publishes the installer when a tag is pushed |
| `.github/workflows/upstream.yml` | Weekly live check against the latest yt-dlp and YouTube |
| `.github/dependabot.yml` | Weekly dependency and GitHub Actions update PRs |

### Pull requests and CI

Changes go through pull requests. Every PR (and every push to `main`) runs:

- `cargo fmt --all --check` — formatting
- `cargo clippy --all-targets --locked -- -D warnings` — lints, with warnings treated as errors
- `cargo test --locked` and a release build
- [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) — security advisories, dependency licenses, banned crates and allowed sources (configured in `deny.toml`)

Run the same checks locally before pushing:

```bash
cargo fmt --all
```

```bash
cargo clippy --all-targets --locked -- -D warnings
```

Dependabot opens weekly PRs for dependency and GitHub Actions updates; merge them once CI passes.

### Upstream check

YouTube and yt-dlp change without notice. Every Monday, `.github/workflows/upstream.yml` runs the live tests in `src/live_tests.rs` with the latest yt-dlp:

- the size estimate in the link preview must pick the same formats yt-dlp picks, for a few long-lived videos and every quality;
- yt-dlp must accept the app's download arguments (checked with `--simulate`, nothing is downloaded).

If the scheduled run fails, it opens an issue labeled `upstream-check` (or comments on the open one). It can also be started by hand from the **Actions** tab. If YouTube asks the runner to confirm it is not a bot, the affected checks are skipped with a warning instead of failing.

To run the live tests locally (needs yt-dlp, Deno and FFmpeg):

```bash
cargo test --release -- --ignored live_
```

### Releasing a version

Releasing is just merging a pull request that bumps the version. On every push to `main`, the release workflow ([.github/workflows/release.yml](.github/workflows/release.yml)) reads the `version` in `Cargo.toml`. If there is no `vX.Y.Z` tag for it yet, it:

1. runs the tests;
2. builds the installer;
3. publishes a GitHub **Release**, which creates the `vX.Y.Z` tag on the merge commit, with the installer, a `SHA256SUMS.txt` file and a changelog of the commits since the previous tag.

If the version already has a tag, nothing happens. To release, for example, version 0.9.0:

1. On a branch, set `version = "0.9.0"` in `Cargo.toml` and run `cargo build --release` to update `Cargo.lock`.
2. Commit, push and open a pull request.
3. Merge it once CI passes.

Don't create release tags by hand: the workflow creates them. Progress shows up in the repository's **Actions** tab, and the installer under **Releases**.

To preview the notes for the commits since the last release:

```bash
powershell -ExecutionPolicy Bypass -File installer\changelog.ps1 -Tag HEAD
```

## License

[MIT](LICENSE) © macedo. If you find it useful and we ever meet, a beer is always welcome. 🍺

YT Downloader is a front end: it doesn't bundle yt-dlp, FFmpeg or Deno, which are installed separately and have their own licenses. You are responsible for respecting the terms of the sites you download from and the rights of the content's owners.
