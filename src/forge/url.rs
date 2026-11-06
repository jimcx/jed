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
