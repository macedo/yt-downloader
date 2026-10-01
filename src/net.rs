//! Shared HTTP client.

pub fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .user_agent(concat!("yt-downloader/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}
