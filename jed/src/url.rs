//! Web links: `jtui::url`'s `url_at`, and `open`, which in tests records the URL instead of
//! launching a browser.

pub use jtui::url::url_at;

/// URLs `open` was given in tests, which never launch a browser.
#[cfg(test)]
pub static OPENED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Whether `open` was given `url` (tests: give each test its own URL, they run in parallel).
#[cfg(test)]
pub fn opened(url: &str) -> bool {
    OPENED.lock().unwrap().iter().any(|u| u == url)
}

/// Opens `url` in the system's browser (`jtui::url::open`); under test records it instead.
/// Blocking: call from spawn_blocking
pub fn open(url: &str) -> Result<(), String> {
    #[cfg(test)]
    {
        OPENED.lock().unwrap().push(url.to_string());
        Ok(())
    }
    #[cfg(not(test))]
    jtui::url::open(url)
}
