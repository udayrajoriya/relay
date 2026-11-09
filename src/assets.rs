use anyhow::Result;
use std::path::Path;
pub const FILES: &[(&str, &[u8])] = &[
    (
        "chromium/background.js",
        include_bytes!("../extension/chromium/background.js"),
    ),
    (
        "chromium/manifest.json",
        include_bytes!("../extension/chromium/manifest.json"),
    ),
    (
        "chromium/popup.css",
        include_bytes!("../extension/chromium/popup.css"),
    ),
    (
        "chromium/popup.html",
        include_bytes!("../extension/chromium/popup.html"),
    ),
    (
        "chromium/popup.js",
        include_bytes!("../extension/chromium/popup.js"),
    ),
    (
        "firefox/background.js",
        include_bytes!("../extension/firefox/background.js"),
    ),
    (
        "firefox/manifest.json",
        include_bytes!("../extension/firefox/manifest.json"),
    ),
    (
        "firefox/popup.css",
        include_bytes!("../extension/firefox/popup.css"),
    ),
    (
        "firefox/popup.html",
        include_bytes!("../extension/firefox/popup.html"),
    ),
    (
        "firefox/popup.js",
        include_bytes!("../extension/firefox/popup.js"),
    ),
];
pub fn extract(root: &Path) -> Result<()> {
    for (name, bytes) in FILES {
        let target = root.join(name);
        std::fs::create_dir_all(target.parent().unwrap())?;
        if std::fs::read(&target).ok().as_deref() != Some(*bytes) {
            std::fs::write(target, bytes)?;
        }
    }
    Ok(())
}
