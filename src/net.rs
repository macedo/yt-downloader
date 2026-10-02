//! Shared HTTP clients.

use std::time::Duration;

/// For small API calls and thumbnails: gives up after 10 seconds.
pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(concat!("yt-downloader/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

/// For downloading files such as the app's installer: a quick connect, then
/// up to 20 minutes for slow connections.
pub fn download_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(20 * 60)))
        .user_agent(concat!("yt-downloader/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}
