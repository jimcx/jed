//! Git remote URL to a `RepoRef`, as `scripts/mr` does: the SSH user and port are dropped
//! (`ssh://git@git.example.com:2200/alice/project.git` is the repository `alice/project` on `git.example.com`).
//! An `http(s)://` remote keeps its port, since that is where the web server listens.

use super::RepoRef;

/// Parses `https://host[:port]/owner/repo[.git]`, `http://…`, `ssh://[user@]host[:port]/owner/repo[.git]`
/// and `[user@]host:owner/repo[.git]`. `None` for a local path or anything else.
pub fn parse_remote(url: &str) -> Option<RepoRef> {
    let url = url.trim();
    let (host, path) = if let Some(rest) = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://")) {
        let (authority, path) = rest.split_once('/')?;
        (strip_user(authority).to_string(), path)
    } else if let Some(rest) = url.strip_prefix("ssh://").or_else(|| url.strip_prefix("git+ssh://")) {
        let (authority, path) = rest.split_once('/')?;
        let host = strip_user(authority);
        (strip_port(host).to_string(), path)
    } else if url.contains("://") || url.starts_with('/') || url.starts_with('.') {
        return None;
    } else {
        // scp-like: [user@]host:owner/repo
        let (authority, path) = url.split_once(':')?;
        if authority.is_empty() || authority.contains('/') {
            return None;
        }
        (strip_user(authority).to_string(), path)
    };
    let path = path.split(['?', '#']).next()?.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || !path.contains('/') || path.split('/').any(str::is_empty) {
        return None;
    }
    Some(RepoRef {
        host: host.to_ascii_lowercase(),
        path: path.to_string(),
    })
}

/// `url` when it is safe to open as a link on `host` (a review's host, lowercase, with its port
/// if it has one): `https://` (or `http://` when `allow_http`), the same authority as `host`
/// (no user info, case ignored), and no whitespace or control characters. The URL is remote
/// text, so anything else is `None`.
pub fn usable_link<'a>(url: &'a str, host: &str, allow_http: bool) -> Option<&'a str> {
    if url.is_empty() || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let scheme_ok = scheme.eq_ignore_ascii_case("https") || (allow_http && scheme.eq_ignore_ascii_case("http"));
    if !scheme_ok {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    (!authority.contains('@') && authority.eq_ignore_ascii_case(host)).then_some(url)
}

fn strip_user(authority: &str) -> &str {
    authority.rsplit_once('@').map_or(authority, |(_, h)| h)
}

/// Drops `:port` (but not the colons inside an IPv6 `[...]` literal).
fn strip_port(host: &str) -> &str {
    match host.rsplit_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        _ => host,
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn usable_link_checks_scheme_and_host() {
        let ok = |u, h, a| usable_link(u, h, a).is_some();
        assert!(ok(
            "https://git.example.com/alice/project/pulls/7",
            "git.example.com",
            false
        ));
        assert!(ok("HTTPS://Git.Example.com/x", "git.example.com", false));
        assert!(ok("https://git.example.com", "git.example.com", false));
        assert!(ok("https://git.example.com:3000/x#c1", "git.example.com:3000", false));
        assert!(ok("http://git.example.com/x", "git.example.com", true));
        assert!(!ok("http://git.example.com/x", "git.example.com", false));
        assert!(!ok("", "git.example.com", true));
        assert!(!ok("https://evil.example.org/x", "git.example.com", false));
        assert!(!ok("https://git.example.com.evil.org/x", "git.example.com", false));
        assert!(!ok("https://git.example.com@evil.org/x", "git.example.com", false));
        assert!(!ok("https://evil.org@git.example.com/x", "git.example.com", false));
        assert!(!ok("https://git.example.com:8080/x", "git.example.com", false));
        assert!(!ok("https://git.example.com/x y", "git.example.com", false));
        assert!(!ok("https://git.example.com/x\u{1b}[0m", "git.example.com", false));
        assert!(!ok("file:///etc/passwd", "git.example.com", true));
        assert!(!ok("javascript:alert(1)", "git.example.com", true));
        assert!(!ok("ftp://git.example.com/x", "git.example.com", true));
        // Host edge cases: prefixes, suffixes, ports, backslashes and user info.
        assert!(!ok("https://evilgit.example.com/x", "git.example.com", false));
        assert!(!ok("https://git.example.com.evil.org", "git.example.com", false));
        assert!(!ok("https://sub.git.example.com/x", "git.example.com", false));
        assert!(!ok("https://git.example.com/x", "git.example.com:3000", false));
        assert!(!ok("https://git.example.com:3000/x", "git.example.com", false));
        assert!(!ok("https://git.example.com:/x", "git.example.com", false));
        assert!(!ok("https://git.example.com\\@evil.org/x", "git.example.com", false));
        assert!(!ok("https://git.example.com\\.evil.org/x", "git.example.com", false));
        assert!(ok("https://git.example.com/a@b", "git.example.com", false));
        assert!(ok("https://git.example.com?q=a@b", "git.example.com", false));
        assert!(ok("https://GIT.EXAMPLE.COM#frag", "git.example.com", false));
        assert!(!ok("https:///git.example.com/x", "git.example.com", false));
        assert!(!ok("git.example.com/x", "git.example.com", false));
    }
    use super::*;

    fn r(host: &str, path: &str) -> Option<RepoRef> {
        Some(RepoRef {
            host: host.into(),
            path: path.into(),
        })
    }

    #[test]
    fn ssh_user_and_port_are_dropped() {
        assert_eq!(
            parse_remote("ssh://git@git.example.com:2200/alice/project.git"),
            r("git.example.com", "alice/project")
        );
        assert_eq!(
            parse_remote("ssh://git.example.com/alice/project"),
            r("git.example.com", "alice/project")
        );
        assert_eq!(
            parse_remote("ssh://git@[::1]:22/alice/project.git"),
            r("[::1]", "alice/project")
        );
    }

    #[test]
    fn scp_style() {
        assert_eq!(
            parse_remote("git@github.com:alice/project.git"),
            r("github.com", "alice/project")
        );
        assert_eq!(
            parse_remote("gitlab.com:group/sub/proj"),
            r("gitlab.com", "group/sub/proj")
        );
    }

    #[test]
    fn https_keeps_port_and_drops_credentials() {
        assert_eq!(
            parse_remote("https://user:pw@Host.example:3000/o/r.git"),
            r("host.example:3000", "o/r")
        );
        assert_eq!(parse_remote("https://github.com/o/r/"), r("github.com", "o/r"));
        assert_eq!(
            parse_remote("http://localhost:3000/o/r.git"),
            r("localhost:3000", "o/r")
        );
    }

    #[test]
    fn rejects_local_paths_and_short_paths() {
        assert_eq!(parse_remote("/srv/git/repo.git"), None);
        assert_eq!(parse_remote("../repo"), None);
        assert_eq!(parse_remote("file:///srv/repo.git"), None);
        assert_eq!(parse_remote("https://host/onlyone"), None);
        assert_eq!(parse_remote(""), None);
    }
}
