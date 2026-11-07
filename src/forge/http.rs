//! The blocking HTTP client every forge adapter uses: `ureq` with rustls and the platform
//! certificate verifier (so a private CA installed in the system works).
//!
//! - HTTPS only, unless the `forges.toml` entry wrote an `http://` API URL itself.
//! - Redirects are never followed (so the token can't be sent to a host the server names): a 3xx
//!   is an error.
//! - The token is sent as the forge's own header and appears in no error, `Debug` or log line.
//! - Text from the server goes through `clean_line` / `clean_text`.
//!
//! Every request is `Blocking: call from spawn_blocking`.

use std::fmt;
use std::time::Duration;

use serde::de::DeserializeOwned;

use super::config::Resolved;
use super::{ForgeError, Kind, Result};

/// A page of a list is requested this large; Gitea caps it at its own maximum (50 by default).
const PAGE_SIZE: usize = 50;
/// Stop paging after this many pages (50 000 items): a server that never ends can't hang jed.
const MAX_PAGES: usize = 1000;
/// GitHub pages hold up to 100 items.
const GITHUB_PAGE_SIZE: usize = 100;
/// The REST API version GitHub is asked for (`X-GitHub-Api-Version`).
const GITHUB_API_VERSION: &str = "2022-11-28";

#[derive(Clone)]
pub struct Client {
    agent: ureq::Agent,
    api_url: String,
    kind: Kind,
    token: Option<String>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("api_url", &self.api_url)
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// A successful answer.
#[derive(Debug)]
pub struct Response {
    pub body: String,
    /// `X-Total-Count`, when the server sends it.
    pub total: Option<usize>,
    /// The URL of a `Link: <…>; rel="next"` header, when there is one (GitHub's paging).
    pub next: Option<String>,
    /// GitLab's `X-Next-Page` (empty or absent on the last page).
    pub next_page: Option<u32>,
}

impl Client {
    /// `lookup` reads an environment variable (injected so tests need not touch the process
    /// environment). A missing token is allowed (a public repository may answer without one).
    ///
    /// Blocking: builds the TLS configuration (loads the platform's certificates); call from
    /// spawn_blocking.
    pub fn new(resolved: &Resolved, lookup: impl Fn(&str) -> Option<String>) -> Result<Client> {
        let api_url = resolved.api_url.trim_end_matches('/').to_string();
        if api_url.starts_with("http://") && !resolved.allow_http {
            return Err(ForgeError::Config(format!(
                "{}: the API URL is not https; write it as api_url in forges.toml to allow it",
                resolved.host
            )));
        }
        if !api_url.starts_with("https://") && !api_url.starts_with("http://") {
            return Err(ForgeError::Config(format!("{}: bad API URL", resolved.host)));
        }
        let token = (resolved.token.as_ref().map(|t| t.expose().to_string()))
            .or_else(|| lookup(&resolved.token_env))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty());
        let tls = ureq::tls::TlsConfig::builder()
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build();
        let config = ureq::Agent::config_builder()
            .https_only(!resolved.allow_http)
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(30)))
            .tls_config(tls)
            .user_agent("jed")
            .build();
        Ok(Client {
            agent: config.into(),
            api_url,
            kind: resolved.kind,
            token,
        })
    }

    /// GET `api_url + path` (`path` starts with `/` and carries its query).
    ///
    /// Blocking: call from spawn_blocking.
    pub fn get(&self, path: &str) -> Result<Response> {
        let request = self.headers(self.agent.get(format!("{}{}", self.api_url, path)));
        self.finish(request.call())
    }

    /// POST `body` (JSON text) to `api_url + path`.
    ///
    /// Blocking: call from spawn_blocking.
    pub fn post(&self, path: &str, body: &str) -> Result<Response> {
        let request = self.headers(self.agent.post(format!("{}{}", self.api_url, path)));
        self.finish(request.content_type("application/json").send(body))
    }

    /// PATCH `body` (JSON text) to `api_url + path`.
    ///
    /// Blocking: call from spawn_blocking.
    pub fn patch(&self, path: &str, body: &str) -> Result<Response> {
        let request = self.headers(self.agent.patch(format!("{}{}", self.api_url, path)));
        self.finish(request.content_type("application/json").send(body))
    }

    /// PUT `body` (JSON text) to `api_url + path`.
    ///
    /// Blocking: call from spawn_blocking.
    pub fn put(&self, path: &str, body: &str) -> Result<Response> {
        let request = self.headers(self.agent.put(format!("{}{}", self.api_url, path)));
        self.finish(request.content_type("application/json").send(body))
    }

    /// DELETE `api_url + path`.
    ///
    /// Blocking: call from spawn_blocking.
    pub fn delete(&self, path: &str) -> Result<Response> {
        let request = self.headers(self.agent.delete(format!("{}{}", self.api_url, path)));
        self.finish(request.call())
    }

    /// `Accept` and the forge's own token header.
    fn headers<B>(&self, mut request: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        request = match self.kind {
            Kind::GitHub => request
                .header("Accept", "application/vnd.github+json")
                .header("X-GitHub-Api-Version", GITHUB_API_VERSION),
            _ => request.header("Accept", "application/json"),
        };
        if let Some(token) = &self.token {
            let (name, value) = match self.kind {
                Kind::Gitea => ("Authorization", format!("token {token}")),
                Kind::GitHub => ("Authorization", format!("Bearer {token}")),
                Kind::GitLab => ("PRIVATE-TOKEN", token.clone()),
            };
            request = request.header(name, &value);
        }
        request
    }

    fn finish(&self, sent: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<Response> {
        let mut response = sent.map_err(|e| ForgeError::Transport(self.scrub(&e.to_string())))?;
        let status = response.status().as_u16();
        let total = response
            .headers()
            .get("x-total-count")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok());
        let next = response
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .and_then(next_link);
        let next_page = response
            .headers()
            .get("x-next-page")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse().ok());
        let rate_limited = matches!(status, 403 | 429)
            && response
                .headers()
                .get("x-ratelimit-remaining")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.trim() == "0");
        if (300..400).contains(&status) {
            return Err(ForgeError::Transport(format!(
                "the server redirected ({status}); redirects are not followed"
            )));
        }
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| ForgeError::Transport(self.scrub(&e.to_string())))?;
        if status >= 400 {
            let mut message = self.scrub(&error_message(&body));
            if rate_limited {
                message = format!("rate limit exceeded; {message}").trim().to_string();
            }
            return Err(ForgeError::Status { status, message });
        }
        Ok(Response {
            body,
            total,
            next,
            next_page,
        })
    }

    /// GET and parse the JSON body.
    ///
    /// Blocking: call from spawn_blocking.
    pub fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let response = self.get(path)?;
        parse_json(&response.body)
    }

    /// GET every page of a list (`path` may carry a query already) and concatenate them.
    ///
    /// Blocking: call from spawn_blocking.
    pub fn get_all<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        if self.kind == Kind::GitHub {
            return self.get_all_linked(path);
        }
        if self.kind == Kind::GitLab {
            return self.get_all_numbered(path);
        }
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut all = Vec::new();
        for page in 1..=MAX_PAGES {
            let response = self.get(&format!("{path}{sep}page={page}&limit={PAGE_SIZE}"))?;
            let items: Vec<T> = parse_json(&response.body)?;
            let got = items.len();
            all.extend(items);
            // A server may cap the page below ours, so stop on an empty page or the announced total.
            if got == 0
                || response.total.is_some_and(|t| all.len() >= t)
                || (response.total.is_none() && got < PAGE_SIZE)
            {
                break;
            }
        }
        Ok(all)
    }

    /// GitLab's paging: `per_page=100` and the page number `X-Next-Page` names (a full page
    /// without the header is followed by one more request, which ends the list when empty).
    fn get_all_numbered<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut all = Vec::new();
        let mut page = 1;
        for _ in 0..MAX_PAGES {
            let response = self.get(&format!("{path}{sep}per_page={GITHUB_PAGE_SIZE}&page={page}"))?;
            let items: Vec<T> = parse_json(&response.body)?;
            let got = items.len();
            all.extend(items);
            match response.next_page {
                Some(n) if n > page => page = n,
                Some(_) => break,
                None if got >= GITHUB_PAGE_SIZE => page += 1,
                None => break,
            }
        }
        Ok(all)
    }

    /// GitHub's paging: 100 per page, then the `rel="next"` link until there is none. Only the
    /// link's query is used, on `path`'s own address, so a link naming another host is never
    /// requested (with the token).
    fn get_all_linked<T: DeserializeOwned>(&self, path: &str) -> Result<Vec<T>> {
        let base = path.split('?').next().unwrap_or(path);
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut target = format!("{path}{sep}per_page={GITHUB_PAGE_SIZE}");
        let mut all = Vec::new();
        for _ in 0..MAX_PAGES {
            let response = self.get(&target)?;
            let items: Vec<T> = parse_json(&response.body)?;
            all.extend(items);
            let Some(next) = response.next else { break };
            let Some((_, query)) = next.split_once('?') else { break };
            let next_target = format!("{base}?{query}");
            if next_target == target {
                break; // a link to the page just read
            }
            target = next_target;
        }
        Ok(all)
    }

    /// POST a GraphQL query to the API's GraphQL endpoint (`/graphql` beside `/api/v3` on an
    /// Enterprise server, beside the REST root on github.com) and returns its `data`. GraphQL
    /// answers 200 with an `errors` array on failure, which becomes an error here.
    ///
    /// Blocking: call from spawn_blocking.
    pub fn graphql(&self, query: &str, variables: serde_json::Value) -> Result<serde_json::Value> {
        let root = self.api_url.strip_suffix("/v3").unwrap_or(&self.api_url);
        let request = self.headers(self.agent.post(format!("{root}/graphql")));
        let body = serde_json::json!({ "query": query, "variables": variables }).to_string();
        let response = self.finish(request.content_type("application/json").send(body.as_str()))?;
        let mut answer: serde_json::Value = parse_json(&response.body)?;
        if let Some(errors) = answer
            .get("errors")
            .and_then(|e| e.as_array())
            .filter(|e| !e.is_empty())
        {
            let messages: Vec<String> = errors
                .iter()
                .filter_map(|e| e.get("message").and_then(|m| m.as_str()))
                .map(|m| self.scrub(m))
                .collect();
            return Err(ForgeError::Status {
                status: 200,
                message: messages.join("; "),
            });
        }
        answer
            .get_mut("data")
            .map(serde_json::Value::take)
            .filter(|d| !d.is_null())
            .ok_or_else(|| ForgeError::Parse("no data in the GraphQL answer".into()))
    }

    /// Removes the token from text that might echo a request.
    fn scrub(&self, text: &str) -> String {
        let text = clean_line(text);
        match &self.token {
            Some(t) => text.replace(t.as_str(), "<token>"),
            None => text,
        }
    }
}

/// The URL of the `rel="next"` entry of a `Link` header.
fn next_link(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (url, params) = part.split_once(';')?;
        let is_next = params.split(';').any(|p| {
            let p = p.trim().replace(' ', "");
            p == "rel=\"next\"" || p == "rel=next"
        });
        let url = url.trim().strip_prefix('<')?.strip_suffix('>')?;
        is_next.then(|| url.to_string())
    })
}

fn parse_json<T: DeserializeOwned>(body: &str) -> Result<T> {
    serde_json::from_str(body).map_err(|e| ForgeError::Parse(e.to_string()))
}

/// The `message` of a forge's JSON error body, else nothing.
fn error_message(body: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str().map(clean_line)))
        .unwrap_or_default()
}

/// Remote text for one line: control characters (including newlines and tabs, which become
/// spaces) and bidirectional overrides removed.
pub fn clean_line(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter_map(|c| match c {
            '\n' | '\t' => Some(' '),
            c if is_unsafe(c) => None,
            c => Some(c),
        })
        .collect();
    cleaned.trim().to_string()
}

/// Remote text of several lines (a comment body): like `clean_line`, but newlines and tabs stay
/// and `\r` is dropped.
pub fn clean_text(s: &str) -> String {
    s.chars().filter(|&c| c == '\n' || c == '\t' || !is_unsafe(c)).collect()
}

fn is_unsafe(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}')
}

/// Percent-encodes one URL path segment.
pub fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::fake::{FakeServer, Reply};
    use super::*;

    fn resolved(base: &str, kind: Kind, allow_http: bool) -> Resolved {
        Resolved {
            host: "fake".into(),
            kind,
            api_url: base.into(),
            token_env: "FAKE_TOKEN".into(),
            token: None,
            allow_http,
        }
    }

    fn client(server: &FakeServer, token: Option<&str>) -> Client {
        let t = token.map(String::from);
        Client::new(&resolved(&server.url(), Kind::Gitea, true), move |_| t.clone()).unwrap()
    }

    #[test]
    fn http_is_refused_unless_the_entry_allows_it() {
        let err = Client::new(&resolved("http://example.invalid/api", Kind::Gitea, false), |_| None).unwrap_err();
        assert!(matches!(err, ForgeError::Config(_)), "{err}");
        assert!(Client::new(&resolved("http://example.invalid/api", Kind::Gitea, true), |_| None).is_ok());
    }

    #[test]
    fn each_forge_gets_its_own_token_header() {
        let server = FakeServer::start(vec![("/x".into(), Reply::json("{}"))]);
        for (kind, name, value) in [
            (Kind::Gitea, "authorization", "token sekrit"),
            (Kind::GitHub, "authorization", "Bearer sekrit"),
            (Kind::GitLab, "private-token", "sekrit"),
        ] {
            let c = Client::new(&resolved(&server.url(), kind, true), |_| Some("sekrit".into())).unwrap();
            c.get("/x").unwrap();
            let last = server.requests().pop().unwrap();
            assert_eq!(last.header(name), Some(value), "{kind:?}");
        }
        let c = client(&server, None);
        c.get("/x").unwrap();
        assert_eq!(server.requests().pop().unwrap().header("authorization"), None);
    }

    #[test]
    fn a_redirect_is_not_followed_and_the_token_goes_nowhere_else() {
        let other = FakeServer::start(vec![("/stolen".into(), Reply::json("{}"))]);
        let server = FakeServer::start(vec![(
            "/r".into(),
            Reply::status(302).with_header("Location", &format!("{}/stolen", other.url())),
        )]);
        let err = client(&server, Some("sekrit")).get("/r").unwrap_err();
        assert!(matches!(err, ForgeError::Transport(_)), "{err}");
        assert_eq!(server.requests().len(), 1);
        assert!(other.requests().is_empty(), "the redirect target was contacted");
    }

    #[test]
    fn errors_carry_the_cleaned_message_and_never_the_token() {
        let server = FakeServer::start(vec![(
            "/e".into(),
            Reply::status(404).with_body("{\"message\":\"no \\u001b[31msuch\\nrepo sekrit\"}"),
        )]);
        let c = client(&server, Some("sekrit"));
        let err = c.get("/e").unwrap_err();
        let ForgeError::Status { status, message } = &err else {
            panic!("{err}")
        };
        assert_eq!(*status, 404);
        assert_eq!(message, "no [31msuch repo <token>");
        assert!(!format!("{c:?}").contains("sekrit"));
        assert_eq!(c.scrub("failed for sekrit"), "failed for <token>");
    }

    #[test]
    fn paging_follows_the_total() {
        let server = FakeServer::start(vec![
            (
                "/l?page=1&limit=50".into(),
                Reply::json("[1,2]").with_header("X-Total-Count", "3"),
            ),
            (
                "/l?page=2&limit=50".into(),
                Reply::json("[3]").with_header("X-Total-Count", "3"),
            ),
        ]);
        let got: Vec<u32> = client(&server, None).get_all("/l").unwrap();
        assert_eq!(got, [1, 2, 3]);
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn github_requests_carry_the_api_headers_and_page_by_link() {
        let link = "<https://elsewhere.invalid/l?page=2&per_page=100>; rel=\"next\", <https://elsewhere.invalid/l?page=9>; rel=\"last\"";
        let server = FakeServer::start(vec![
            ("/l?per_page=100".into(), Reply::json("[1,2]").with_header("Link", link)),
            ("/l?page=2&per_page=100".into(), Reply::json("[3]")),
        ]);
        let c = Client::new(&resolved(&server.url(), Kind::GitHub, true), |_| Some("sekrit".into())).unwrap();
        let got: Vec<u32> = c.get_all("/l").unwrap();
        assert_eq!(got, [1, 2, 3]);
        let seen = server.requests();
        assert_eq!(seen.len(), 2, "the link's host is never contacted");
        assert_eq!(seen[0].header("accept"), Some("application/vnd.github+json"));
        assert_eq!(seen[0].header("x-github-api-version"), Some("2022-11-28"));
        assert_eq!(seen[0].header("user-agent"), Some("jed"));
        assert_eq!(seen[0].header("authorization"), Some("Bearer sekrit"));
    }

    #[test]
    fn a_link_that_repeats_the_page_stops_the_paging() {
        let server = FakeServer::start(vec![(
            "/l".into(),
            Reply::json("[1]").with_header("Link", "<http://x.invalid/l?per_page=100>; rel=\"next\""),
        )]);
        let c = Client::new(&resolved(&server.url(), Kind::GitHub, true), |_| None).unwrap();
        let got: Vec<u32> = c.get_all("/l").unwrap();
        assert_eq!(got, [1]);
    }

    #[test]
    fn graphql_errors_surface_and_rate_limits_are_named() {
        let server = FakeServer::start(vec![
            (
                "POST /graphql".into(),
                Reply::json("{\"data\":null,\"errors\":[{\"message\":\"bad \\u001b[1mthing sekrit\"}]}"),
            ),
            (
                "/limited".into(),
                Reply::status(403)
                    .with_header("X-RateLimit-Remaining", "0")
                    .with_body("{\"message\":\"API rate limit exceeded\"}"),
            ),
        ]);
        let c = Client::new(&resolved(&server.url(), Kind::GitHub, true), |_| Some("sekrit".into())).unwrap();
        let err = c.graphql("query { x }", serde_json::json!({})).unwrap_err();
        assert_eq!(err.to_string(), "server answered 200: bad [1mthing <token>");
        let err = c.get("/limited").unwrap_err();
        assert!(
            err.to_string().contains("rate limit exceeded; API rate limit exceeded"),
            "{err}"
        );
        assert_eq!(next_link("<a>; rel=\"prev\""), None);
    }

    #[test]
    fn cleaning_removes_control_sequences() {
        assert_eq!(clean_line("a\u{1b}[2Jb\nc\t\u{7}d\u{202E}"), "a[2Jb c d");
        assert_eq!(clean_text("a\r\nb\u{1b}\tc\u{9b}"), "a\nb\tc");
        assert_eq!(encode_segment("a b/ü"), "a%20b%2F%C3%BC");
    }
}
