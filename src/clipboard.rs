//! The system clipboard behind one interface: `arboard` locally, OSC 52 over SSH
//! or when `arboard` fails. Remembers whole-line copies so pasting them back
//! inserts above the cursor's line, as VS Code does.

use std::io::Write;

// Tests build the app with `Clipboard::memory()`, so the real backends go unused there.
#[cfg_attr(test, allow(dead_code))]
enum Backend {
    /// `arboard`, opened on first use so startup doesn't pay for it.
    /// `None` inside means opening failed; OSC 52 from then on.
    Local(Option<Option<arboard::Clipboard>>),
    /// Over SSH: copy through the terminal; paste only arrives as bracketed paste.
    Osc52,
    /// Tests: never touches the real clipboard.
    #[cfg(test)]
    Memory(Option<String>),
}

pub struct Clipboard {
    backend: Backend,
    /// Text of the last whole-line copy or cut (newlines as `\n`), if that was the last copy.
    line_copy: Option<String>,
}

#[derive(Debug)]
pub enum ReadError {
    /// No local clipboard (SSH, or `arboard` unavailable): paste with the terminal instead.
    Unavailable,
    Failed(String),
}

impl Clipboard {
    #[cfg_attr(test, allow(dead_code))]
    pub fn system() -> Self {
        let backend = if std::env::var_os("SSH_TTY").is_some() {
            Backend::Osc52
        } else {
            Backend::Local(None)
        };
        Self {
            backend,
            line_copy: None,
        }
    }

    #[cfg(test)]
    pub fn memory() -> Self {
        Self {
            backend: Backend::Memory(None),
            line_copy: None,
        }
    }

    /// `whole_line`: the text is whole lines copied from empty selections.
    pub fn write(&mut self, text: String, whole_line: bool) {
        self.line_copy = whole_line.then(|| text.clone());
        match &mut self.backend {
            Backend::Local(slot) => {
                let local = slot.get_or_insert_with(|| {
                    arboard::Clipboard::new()
                        .inspect_err(|e| tracing::warn!("system clipboard unavailable, using OSC 52: {e}"))
                        .ok()
                });
                match local.as_mut().map(|c| c.set_text(text.as_str())) {
                    Some(Ok(())) => {}
                    Some(Err(e)) => {
                        tracing::warn!("clipboard write failed, using OSC 52: {e}");
                        osc52(&text);
                    }
                    None => osc52(&text),
                }
            }
            Backend::Osc52 => osc52(&text),
            #[cfg(test)]
            Backend::Memory(m) => *m = Some(text),
        }
    }

    pub fn read(&mut self) -> Result<String, ReadError> {
        match &mut self.backend {
            Backend::Local(Some(None)) | Backend::Osc52 => Err(ReadError::Unavailable),
            Backend::Local(slot) => {
                let local = slot.get_or_insert_with(|| arboard::Clipboard::new().ok());
                let Some(c) = local.as_mut() else {
                    return Err(ReadError::Unavailable);
                };
                c.get_text().map_err(|e| ReadError::Failed(e.to_string()))
            }
            #[cfg(test)]
            Backend::Memory(m) => Ok(m.clone().unwrap_or_default()),
        }
    }

    /// Whether pasted `text` is the last whole-line copy, so it goes above the cursor's line.
    /// Compared with newlines normalized: terminals may turn them into `\r` in a bracketed paste.
    pub fn is_line_copy(&self, text: &str) -> bool {
        self.line_copy
            .as_deref()
            .is_some_and(|l| l == text.replace("\r\n", "\n").replace('\r', "\n"))
    }
}

/// Sets the terminal's clipboard. Wrapped for tmux so it reaches the outer terminal
/// (needs `set -g allow-passthrough on`, or tmux's own `set-clipboard on` handles the plain form).
fn osc52(text: &str) {
    let seq = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
    let seq = if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;{}\x1b\\", seq.replace('\x1b', "\x1b\x1b"))
    } else {
        seq
    };
    let mut out = std::io::stdout();
    if let Err(e) = out.write_all(seq.as_bytes()).and_then(|()| out.flush()) {
        tracing::warn!("OSC 52 write failed: {e}");
    }
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | ((*b as u32) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc_4648_vectors() {
        let cases = [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ];
        for (input, expected) in cases {
            assert_eq!(base64(input.as_bytes()), expected, "{input:?}");
        }
    }

    #[test]
    fn line_copy_mark_follows_the_last_copy() {
        let mut c = Clipboard::memory();
        c.write("a\nb\n".into(), true);
        assert!(c.is_line_copy("a\nb\n"));
        assert!(c.is_line_copy("a\r\nb\r"), "terminal newlines are normalized");
        assert!(!c.is_line_copy("a\n"));
        c.write("a\nb\n".into(), false);
        assert!(!c.is_line_copy("a\nb\n"));
    }
}
