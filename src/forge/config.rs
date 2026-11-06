//! `~/.config/jed/forges.toml`: which hosts are forges, of which kind, where their API is and
//! which environment variable holds the token.
//!
//! ```toml
//! [[forge]]
//! host = "git.example.com"
//! kind = "gitea"                       # gitea | github | gitlab
//! api_url = "https://git.example.com/api/v1"   # optional; http:// only if written out here
//! token_env = "GITEA_TOKEN"            # optional; default GITEA_TOKEN / GITHUB_TOKEN / GITLAB_TOKEN
//! ```
//!
//! `github.com` and `gitlab.com` need no entry. This file is the only source of a host, API URL
//! or token variable: `.workspace` and the settings never are, since a repository must not be
//! able to point jed's token at a server it picks. The file is ignored (with an error shown)
//! when group or other can write to it. The token itself is never in the file.

use std::path::Path;

use serde::Deserialize;

use super::Kind;

pub const FILE_NAME: &str = "forges.toml";

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct RawEntry {
    host: String,
    kind: String,
    api_url: Option<String>,
    token_env: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
struct RawFile {
    #[serde(default)]
    forge: Vec<RawEntry>,
}

/// A host the user configured.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Lower case, with the port for an `http(s)://` remote.
    pub host: String,
    pub kind: Kind,
    /// Without a trailing slash.
    pub api_url: Option<String>,
    pub token_env: Option<String>,
}

/// An entry (or a detected host) with every default filled in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub host: String,
    pub kind: Kind,
    /// The API root, without a trailing slash.
    pub api_url: String,
    pub token_env: String,
    /// The entry wrote an `http://` API URL: the only way to talk unencrypted.
    pub allow_http: bool,
}

#[derive(Clone, Debug, Default)]
pub struct ForgeConfig {
    pub entries: Vec<Entry>,
    /// Problems found while loading, for the user (not fatal).
    pub errors: Vec<String>,
}

/// Loads `forges.toml` from the config directory. A missing file is an empty config.
///
/// Blocking: reads a file; call from spawn_blocking.
pub fn load_default() -> ForgeConfig {
    load(&crate::paths::config_dir().join(FILE_NAME))
}

/// Blocking: reads a file; call from spawn_blocking.
pub fn load(path: &Path) -> ForgeConfig {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ForgeConfig::default(),
        Err(e) => return ForgeConfig::error(format!("{FILE_NAME}: {e}")),
    };
    if let Some(problem) = unsafe_permissions(path) {
        return ForgeConfig::error(format!("{FILE_NAME}: ignored, {problem}"));
    }
    parse(&text)
}

#[cfg(unix)]
fn unsafe_permissions(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::metadata(path).ok()?;
    if meta.mode() & 0o022 != 0 {
        return Some("it is writable by group or others (chmod go-w)".into());
    }
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    (meta.uid() != me && me != 0).then(|| "it belongs to another user".into())
}

#[cfg(not(unix))]
fn unsafe_permissions(_: &Path) -> Option<String> {
    None
}

/// Parses the text of a `forges.toml`.
pub fn parse(text: &str) -> ForgeConfig {
    let raw: RawFile = match toml::from_str(text) {
        Ok(r) => r,
        Err(e) => return ForgeConfig::error(format!("{FILE_NAME}: {}", e.message().trim_end())),
    };
    let mut config = ForgeConfig::default();
    for (i, e) in raw.forge.into_iter().enumerate() {
        match entry(e) {
            Ok(e) => config.entries.push(e),
            Err(m) => config.errors.push(format!("{FILE_NAME}: [[forge]] #{}: {m}", i + 1)),
        }
    }
    config
}

fn entry(e: RawEntry) -> Result<Entry, String> {
    let host = e.host.trim().to_ascii_lowercase();
    if host.is_empty() || host.contains(['/', '@', ' ']) {
        return Err(format!("`host` must be a bare host name, got `{}`", e.host));
    }
    let kind =
        Kind::from_name(&e.kind).ok_or_else(|| format!("unknown `kind` `{}` (gitea, github or gitlab)", e.kind))?;
    let api_url = match e.api_url {
        None => None,
        Some(u) => {
            let u = u.trim().trim_end_matches('/').to_string();
            if !(u.starts_with("https://") || u.starts_with("http://")) || u.contains(['@', ' ']) {
                return Err(format!(
                    "`api_url` must be an https:// (or http://) URL without credentials, got `{u}`"
                ));
            }
            Some(u)
        }
    };
    let token_env = match e.token_env {
        None => None,
        Some(v) => {
            let v = v.trim().to_string();
            let ok = !v.is_empty() && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
            if !ok {
                return Err(format!("`token_env` must be an environment variable name, got `{v}`"));
            }
            Some(v)
        }
    };
    Ok(Entry {
        host,
        kind,
        api_url,
        token_env,
    })
}

impl ForgeConfig {
    fn error(message: String) -> ForgeConfig {
        ForgeConfig {
            entries: Vec::new(),
            errors: vec![message],
        }
    }

    /// The forge for `host` (case-insensitive, port included for `http(s)://` remotes): its entry,
    /// else `github.com` / `gitlab.com`. `None` for an unknown host.
    pub fn resolve(&self, host: &str) -> Option<Resolved> {
        let host = host.to_ascii_lowercase();
        let found = self
            .entries
            .iter()
            .find(|e| e.host == host)
            .cloned()
            .or_else(|| detected(&host))?;
        let hostname = host.split(':').next().unwrap_or(&host);
        let api_url = found.api_url.clone().unwrap_or_else(|| match (found.kind, hostname) {
            (Kind::GitHub, "github.com") => "https://api.github.com".to_string(),
            (Kind::GitHub, _) => format!("https://{host}/api/v3"),
            (Kind::Gitea, _) => format!("https://{host}/api/v1"),
            (Kind::GitLab, _) => format!("https://{host}/api/v4"),
        });
        Some(Resolved {
            allow_http: api_url.starts_with("http://") && found.api_url.is_some(),
            token_env: found
                .token_env
                .unwrap_or_else(|| found.kind.default_token_env().to_string()),
            host,
            kind: found.kind,
            api_url,
        })
    }
}

fn detected(host: &str) -> Option<Entry> {
    let kind = match host {
        "github.com" => Kind::GitHub,
        "gitlab.com" => Kind::GitLab,
        _ => return None,
    };
    Some(Entry {
        host: host.to_string(),
        kind,
        api_url: None,
        token_env: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_and_fills_defaults() {
        let c = parse(
            "[[forge]]\nhost = \"Git.Example.com\"\nkind = \"gitea\"\n\n\
             [[forge]]\nhost = \"gl.corp\"\nkind = \"gitlab\"\napi_url = \"https://gl.corp/api/v4/\"\ntoken_env = \"CORP_TOKEN\"\n",
        );
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let g = c.resolve("git.example.com").unwrap();
        assert_eq!(g.api_url, "https://git.example.com/api/v1");
        assert_eq!(g.token_env, "GITEA_TOKEN");
        assert!(!g.allow_http);
        let l = c.resolve("GL.corp").unwrap();
        assert_eq!(
            (l.api_url.as_str(), l.token_env.as_str()),
            ("https://gl.corp/api/v4", "CORP_TOKEN")
        );
    }

    #[test]
    fn github_and_gitlab_com_are_detected_unknown_hosts_are_not() {
        let c = ForgeConfig::default();
        let g = c.resolve("github.com").unwrap();
        assert_eq!(
            (g.kind, g.api_url.as_str(), g.token_env.as_str()),
            (Kind::GitHub, "https://api.github.com", "GITHUB_TOKEN")
        );
        let l = c.resolve("gitlab.com").unwrap();
        assert_eq!(
            (l.kind, l.api_url.as_str()),
            (Kind::GitLab, "https://gitlab.com/api/v4")
        );
        assert!(c.resolve("git.example.com").is_none());
    }

    #[test]
    fn http_needs_an_explicit_api_url() {
        let c = parse(
            "[[forge]]\nhost = \"localhost:3000\"\nkind = \"gitea\"\napi_url = \"http://localhost:3000/api/v1\"\n",
        );
        assert!(c.resolve("localhost:3000").unwrap().allow_http);
        let c = parse("[[forge]]\nhost = \"localhost:3000\"\nkind = \"gitea\"\n");
        assert!(!c.resolve("localhost:3000").unwrap().allow_http);
    }

    #[test]
    fn bad_entries_are_reported_and_skipped() {
        let c = parse(
            "[[forge]]\nhost = \"a\"\nkind = \"svn\"\n[[forge]]\nhost = \"b\"\nkind = \"gitea\"\ntoken_env = \"X Y\"\n\
             [[forge]]\nhost = \"c\"\nkind = \"gitea\"\napi_url = \"ftp://c\"\n[[forge]]\nhost = \"d\"\nkind = \"gitea\"\n",
        );
        assert_eq!(c.errors.len(), 3, "{:?}", c.errors);
        assert_eq!(c.entries.len(), 1);
        let c = parse("[[forge]]\nhost = \"a\"\nkind = \"gitea\"\ntoken = \"secret\"\n");
        assert!(
            c.entries.is_empty() && c.errors.len() == 1,
            "unknown keys (a token) are refused: {:?}",
            c.errors
        );
    }

    #[test]
    fn a_workspace_file_cannot_name_a_forge() {
        use crate::workspace::config::WorkspaceConfig;
        for text in [
            "[[forge]]\nhost = \"evil.example\"\nkind = \"gitea\"\n",
            "forge_host = \"evil.example\"\n",
            "api_url = \"https://evil.example\"\n",
            "token_env = \"HOME\"\n",
            "[forge]\nhost = \"evil.example\"\n",
        ] {
            assert!(WorkspaceConfig::parse(text).is_err(), "accepted: {text}");
        }
        let s: Result<crate::config::settings::SettingsOverrides, _> =
            toml::from_str("api_url = \"https://evil.example\"");
        assert!(s.is_err());
    }

    #[test]
    fn credentials_in_host_or_api_url_are_refused() {
        let c = parse(
            "[[forge]]\nhost = \"u@h\"\nkind = \"gitea\"\n[[forge]]\nhost = \"h\"\nkind = \"gitea\"\napi_url = \"https://u:p@h/api\"\n",
        );
        assert!(c.entries.is_empty() && c.errors.len() == 2, "{:?}", c.errors);
    }

    #[cfg(unix)]
    #[test]
    fn a_file_others_can_write_is_ignored() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("jed-forges-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, "[[forge]]\nhost = \"a\"\nkind = \"gitea\"\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(load(&path).entries.len(), 1);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let c = load(&path);
        assert!(
            c.entries.is_empty() && c.errors[0].contains("writable"),
            "{:?}",
            c.errors
        );
        assert!(load(&dir.join("missing.toml")).errors.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
