//! `file://` URIs for paths, as servers expect them (percent-encoded UTF-8 bytes).

use std::path::{Path, PathBuf};

use lsp_types::Uri;

/// The `file://` URI of an absolute path.
pub fn from_path(path: &Path) -> Uri {
    let mut s = String::from("file://");
    for &b in path.as_os_str().as_encoded_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s.parse().expect("percent-encoded file URI")
}

/// The path of a `file://` URI; `None` for other schemes or a malformed escape.
pub fn to_path(uri: &Uri) -> Option<PathBuf> {
    let rest = uri.as_str().strip_prefix("file://")?;
    // `file://host/path`: only an empty or `localhost` host is ours.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(out).ok().map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_round_trip() {
        let path = Path::new("/home/a b/c#d/é%.cpp");
        let uri = from_path(path);
        assert_eq!(uri.as_str(), "file:///home/a%20b/c%23d/%C3%A9%25.cpp");
        assert_eq!(to_path(&uri).as_deref(), Some(path));
        let other: Uri = "https://example.com/x".parse().unwrap();
        assert_eq!(to_path(&other), None);
        let lower: Uri = "file:///a/%c3%a9".parse().unwrap();
        assert_eq!(to_path(&lower), Some(PathBuf::from("/a/é")));
    }
}
