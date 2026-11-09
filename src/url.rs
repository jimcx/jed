//! Web links in text: the `http(s)://` URL around a byte offset of a line (what a Cmd/Ctrl+click
//! on a terminal or editor opens), and handing one to the system's browser.
//!
//! A URL runs from its scheme to the first whitespace, control or non-ASCII character (box
//! drawing borders of terminal UIs end it) or one of `< > " ' \` { } | \ ^`; trailing sentence
//! punctuation and a closing bracket it didn't open are left out. Only `http` and `https` are
//! recognised, so a click never launches some other program through a custom scheme.

/// The URL in `line` that byte `at` falls in, if any.
pub fn url_at(line: &str, at: usize) -> Option<&str> {
    let mut from = 0;
    while let Some(i) = line[from..].find("http") {
        let start = from + i;
        let rest = &line[start..];
        let scheme = ["https://", "http://"].into_iter().find(|s| rest.starts_with(s));
        let Some(scheme) = scheme else {
            from = start + 4;
            continue;
        };
        let len = rest.find(|c: char| !is_url_char(c)).unwrap_or(rest.len());
        let url = trim_end(&rest[..len]);
        let end = start + url.len();
        if url.len() > scheme.len() && (start..end).contains(&at) {
            return Some(url);
        }
        if start > at {
            return None;
        }
        from = start + len.max(scheme.len());
    }
    None
}

fn is_url_char(c: char) -> bool {
    c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | '\'' | '`' | '{' | '}' | '|' | '\\' | '^')
}

/// `url` without trailing punctuation, or a `)` / `]` with no opening one inside it.
fn trim_end(mut url: &str) -> &str {
    loop {
        let unbalanced = |open, close| url.ends_with(close) && url.matches(open).count() < url.matches(close).count();
        if url.ends_with(['.', ',', ';', ':', '!', '?']) || unbalanced('(', ')') || unbalanced('[', ']') {
            url = &url[..url.len() - 1];
        } else {
            return url;
        }
    }
}

/// URLs `open` was given in tests, which never launch a browser.
#[cfg(test)]
pub static OPENED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Whether `open` was given `url` (tests: give each test its own URL, they run in parallel).
#[cfg(test)]
pub fn opened(url: &str) -> bool {
    OPENED.lock().unwrap().iter().any(|u| u == url)
}

/// Opens `url` in the system's browser (`open` on macOS, `xdg-open` elsewhere).
/// Blocking: call from spawn_blocking
pub fn open(url: &str) -> Result<(), String> {
    if cfg!(test) {
        #[cfg(test)]
        OPENED.lock().unwrap().push(url.to_string());
        return Ok(());
    }
    let program = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let status = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("{program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} exited with {status}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: &str, needle: &str) -> Option<String> {
        url_at(line, line.find(needle).unwrap()).map(str::to_string)
    }

    #[test]
    fn finds_the_url_under_the_offset() {
        let line = "see https://example.com/a?b=1&c=%20#top and http://x.org/y.";
        assert_eq!(
            at(line, "example").as_deref(),
            Some("https://example.com/a?b=1&c=%20#top")
        );
        assert_eq!(
            at(line, "https").as_deref(),
            Some("https://example.com/a?b=1&c=%20#top")
        );
        assert_eq!(at(line, "#top").as_deref(), Some("https://example.com/a?b=1&c=%20#top"));
        assert_eq!(
            at(line, "x.org").as_deref(),
            Some("http://x.org/y"),
            "trailing `.` dropped"
        );
        assert_eq!(at(line, "see"), None);
        assert_eq!(at(line, " and"), None);
    }

    #[test]
    fn brackets_quotes_and_borders() {
        assert_eq!(at("(see https://a.io/x)", "a.io").as_deref(), Some("https://a.io/x"));
        assert_eq!(
            at("https://en.wikipedia.org/wiki/Rust_(language))", "wiki").as_deref(),
            Some("https://en.wikipedia.org/wiki/Rust_(language)")
        );
        assert_eq!(at("[link](https://a.io/b)", "a.io").as_deref(), Some("https://a.io/b"));
        assert_eq!(at("\"https://a.io/q\",", "a.io").as_deref(), Some("https://a.io/q"));
        assert_eq!(at("│ https://a.io/q │", "a.io").as_deref(), Some("https://a.io/q"));
        assert_eq!(at("<https://a.io/q>", "a.io").as_deref(), Some("https://a.io/q"));
    }

    #[test]
    fn needs_a_web_scheme_and_a_host() {
        assert_eq!(at("http:// x", "http"), None);
        assert_eq!(at("https:", "https"), None);
        assert_eq!(at("httpx://a.io", "a.io"), None);
        assert_eq!(at("ftp://a.io/f", "a.io"), None);
        assert_eq!(at("javascript:alert(1)", "alert"), None);
        assert_eq!(at("http http https://a.io", "a.io").as_deref(), Some("https://a.io"));
    }
}
