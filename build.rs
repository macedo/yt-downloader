// Embeds the app icon and version information in the Windows executable, so
// shortcuts, the taskbar and the file's Properties > Details show them.
// The version fields come from Cargo.toml.
fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "YT Downloader")
            .set("FileDescription", "YT Downloader")
            .set("LegalCopyright", "Copyright (c) 2026 macedo. MIT License.");
        res.compile()
            .expect("embed the icon and version information in the executable");
    }
}
