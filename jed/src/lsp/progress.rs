//! Work-done progress a server reports (`$/progress` with a `begin` / `report` / `end` value per
//! token, after `window/workDoneProgress/create`): what the status bar shows while a server is
//! busy, e.g. rust-analyzer indexing or running `cargo check` ("rust-analyzer: Indexing 40%").

use serde_json::Value;

/// One piece of work in progress.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Work {
    pub title: String,
    pub message: Option<String>,
    /// 0–100, when the server reports one.
    pub percentage: Option<u32>,
}

impl Work {
    /// `Indexing 40% 3/25 (core)`: the title, the percentage, the message.
    pub fn label(&self) -> String {
        let percentage = self.percentage.map(|p| format!("{p}%"));
        [
            Some(self.title.trim()),
            percentage.as_deref(),
            self.message.as_deref().map(str::trim),
        ]
        .into_iter()
        .flatten()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
    }
}

/// One server's work in progress, by token.
#[derive(Debug, Default)]
pub struct Progress {
    /// Begun and not ended, oldest first, by token (a string or a number: `1` isn't `"1"`).
    active: Vec<(Value, Work)>,
    /// How many `begin`s came (tests: the server reported progress at all).
    begun: usize,
}

impl Progress {
    /// Takes in a `$/progress`'s params. `begin` starts (or restarts) the token's work, `report`
    /// updates its message / percentage (a field left out keeps its value; an unknown token is
    /// ignored), `end` drops it. True when what `latest` shows may have changed.
    pub fn update(&mut self, params: &Value) -> bool {
        let token = match params.get("token") {
            Some(t @ (Value::String(_) | Value::Number(_))) => t.clone(),
            _ => return false,
        };
        let value = params.get("value").unwrap_or(&Value::Null);
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(String::from);
        let percentage = || {
            value
                .get("percentage")
                .and_then(Value::as_f64)
                .map(|p| p.clamp(0.0, 100.0) as u32)
        };
        match value.get("kind").and_then(Value::as_str) {
            Some("begin") => {
                self.active.retain(|(t, _)| *t != token);
                self.active.push((
                    token,
                    Work {
                        title: text("title").unwrap_or_default(),
                        message: text("message"),
                        percentage: percentage(),
                    },
                ));
                self.begun += 1;
                true
            }
            Some("report") => {
                let Some((_, work)) = self.active.iter_mut().find(|(t, _)| *t == token) else {
                    return false;
                };
                if let Some(m) = text("message") {
                    work.message = Some(m);
                }
                if let Some(p) = percentage() {
                    work.percentage = Some(p);
                }
                true
            }
            Some("end") => {
                let before = self.active.len();
                self.active.retain(|(t, _)| *t != token);
                self.active.len() != before
            }
            _ => false,
        }
    }

    /// The work begun last that hasn't ended.
    pub fn latest(&self) -> Option<&Work> {
        self.active.last().map(|(_, w)| w)
    }

    /// Drops all work (the server stopped).
    pub fn clear(&mut self) {
        self.active.clear();
    }

    #[cfg_attr(not(test), allow(dead_code))] // tests
    pub fn begun(&self) -> usize {
        self.begun
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn begin(token: Value, title: &str, percentage: Option<u32>) -> Value {
        json!({ "token": token, "value": { "kind": "begin", "title": title, "percentage": percentage } })
    }

    fn report(token: Value, message: Option<&str>, percentage: Option<u32>) -> Value {
        json!({ "token": token, "value": { "kind": "report", "message": message, "percentage": percentage } })
    }

    fn end(token: Value) -> Value {
        json!({ "token": token, "value": { "kind": "end" } })
    }

    fn shown(p: &Progress) -> Option<String> {
        p.latest().map(Work::label)
    }

    #[test]
    fn begin_report_end() {
        let mut p = Progress::default();
        assert_eq!(shown(&p), None);
        assert!(p.update(&begin(json!("rustAnalyzer/Indexing"), "Indexing", Some(0))));
        assert_eq!(shown(&p).as_deref(), Some("Indexing 0%"));
        assert!(p.update(&report(json!("rustAnalyzer/Indexing"), Some("3/25 (core)"), Some(40))));
        assert_eq!(shown(&p).as_deref(), Some("Indexing 40% 3/25 (core)"));
        // A report without a percentage keeps the last one; without a message, the message.
        assert!(p.update(&report(json!("rustAnalyzer/Indexing"), Some("4/25 (alloc)"), None)));
        assert_eq!(shown(&p).as_deref(), Some("Indexing 40% 4/25 (alloc)"));
        assert!(p.update(&report(json!("rustAnalyzer/Indexing"), None, Some(52))));
        assert_eq!(shown(&p).as_deref(), Some("Indexing 52% 4/25 (alloc)"));
        assert!(p.update(&end(json!("rustAnalyzer/Indexing"))));
        assert_eq!(shown(&p), None);
        assert_eq!(p.begun(), 1);
    }

    #[test]
    fn the_latest_begun_is_shown_until_it_ends() {
        let mut p = Progress::default();
        p.update(&begin(json!(1), "Fetching", None));
        p.update(&begin(json!("check"), "cargo check", None));
        assert_eq!(shown(&p).as_deref(), Some("cargo check"));
        // A report to the older one doesn't bring it forward.
        p.update(&report(json!(1), Some("metadata"), None));
        assert_eq!(shown(&p).as_deref(), Some("cargo check"));
        p.update(&end(json!("check")));
        assert_eq!(shown(&p).as_deref(), Some("Fetching metadata"));
        // Numeric and string tokens are different tokens.
        assert!(!p.update(&end(json!("1"))));
        assert!(p.update(&end(json!(1))));
        assert_eq!(shown(&p), None);
        // Beginning a token again replaces its work.
        p.update(&begin(json!("t"), "One", None));
        p.update(&begin(json!("t"), "Two", Some(150)));
        assert_eq!(shown(&p).as_deref(), Some("Two 100%"));
        p.clear();
        assert_eq!(shown(&p), None);
        assert_eq!(p.begun(), 4);
    }

    #[test]
    fn unknown_tokens_and_bad_params_are_ignored() {
        let mut p = Progress::default();
        assert!(!p.update(&report(json!("x"), Some("m"), Some(5))));
        assert!(!p.update(&end(json!("x"))));
        assert!(!p.update(&json!({ "value": { "kind": "begin", "title": "No token" } })));
        assert!(!p.update(&json!({ "token": "t", "value": { "kind": "other" } })));
        assert!(!p.update(&json!({ "token": "t" })));
        assert_eq!(shown(&p), None);
        // A begin without a title still counts (shown as its percentage / message).
        p.update(&json!({ "token": "t", "value": { "kind": "begin", "message": " loading " } }));
        assert_eq!(shown(&p).as_deref(), Some("loading"));
    }
}
