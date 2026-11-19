//! `printf FORMAT [ARG…]`: `%s`, `%d`, `%%` with `-` / `0` flags and a width; the format's escapes
//! `\n \r \t \a \b \f \v \e \\ \" \xHH` and octal `\033` (also `\0033`). Like the real one, the
//! format is reused while arguments are left.

enum Piece {
    Lit(Vec<u8>),
    Spec {
        left: bool,
        zero: bool,
        width: usize,
        conv: char,
    },
}

/// The bytes `printf` prints for `args` (the format first).
pub fn printf(args: &[String]) -> Result<Vec<u8>, String> {
    let Some((format, args)) = args.split_first() else {
        return Err("usage: printf format [arguments]".into());
    };
    let pieces = parse(format)?;
    let takes = pieces
        .iter()
        .filter(|p| matches!(p, Piece::Spec { conv, .. } if *conv != '%'))
        .count();
    let mut out = Vec::new();
    let mut args = args.iter();
    loop {
        for piece in &pieces {
            match piece {
                Piece::Lit(bytes) => out.extend_from_slice(bytes),
                Piece::Spec { conv: '%', .. } => out.push(b'%'),
                Piece::Spec {
                    left,
                    zero,
                    width,
                    conv,
                } => {
                    let arg = args.next().map(String::as_str).unwrap_or("");
                    let text = match conv {
                        'd' => parse_int(arg)?.to_string(),
                        _ => arg.to_string(),
                    };
                    pad(&mut out, &text, *left, *zero && *conv == 'd', *width);
                }
            }
        }
        if takes == 0 || args.len() == 0 {
            return Ok(out);
        }
    }
}

fn parse_int(arg: &str) -> Result<i64, String> {
    if arg.is_empty() {
        return Ok(0);
    }
    arg.parse().map_err(|_| format!("printf: {arg}: invalid number"))
}

fn pad(out: &mut Vec<u8>, text: &str, left: bool, zero: bool, width: usize) {
    let fill = width.saturating_sub(text.chars().count());
    if left {
        out.extend_from_slice(text.as_bytes());
        out.resize(out.len() + fill, b' ');
    } else if zero {
        let (sign, digits) = text.strip_prefix('-').map_or(("", text), |d| ("-", d));
        out.extend_from_slice(sign.as_bytes());
        out.resize(out.len() + fill, b'0');
        out.extend_from_slice(digits.as_bytes());
    } else {
        out.resize(out.len() + fill, b' ');
        out.extend_from_slice(text.as_bytes());
    }
}

fn parse(format: &str) -> Result<Vec<Piece>, String> {
    let bytes = format.as_bytes();
    let mut pieces = Vec::new();
    let mut lit = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = escape(bytes, i + 1, &mut lit),
            b'%' => {
                i += 1;
                let (mut left, mut zero) = (false, false);
                while let Some(&f @ (b'-' | b'0')) = bytes.get(i) {
                    if f == b'-' {
                        left = true;
                    } else {
                        zero = true;
                    }
                    i += 1;
                }
                let start = i;
                while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                    i += 1;
                }
                let width = format[start..i].parse().unwrap_or(0);
                let conv = match bytes.get(i) {
                    Some(&c @ (b's' | b'd' | b'%')) => c as char,
                    Some(&c) => return Err(format!("printf: %{}: unsupported conversion", c as char)),
                    None => return Err("printf: missing conversion".into()),
                };
                i += 1;
                if !lit.is_empty() {
                    pieces.push(Piece::Lit(std::mem::take(&mut lit)));
                }
                pieces.push(Piece::Spec {
                    left,
                    zero,
                    width,
                    conv,
                });
            }
            b => {
                lit.push(b);
                i += 1;
            }
        }
    }
    if !lit.is_empty() {
        pieces.push(Piece::Lit(lit));
    }
    Ok(pieces)
}

/// Appends the byte(s) of the escape whose letter is at `bytes[i]`; returns the index after it.
fn escape(bytes: &[u8], i: usize, out: &mut Vec<u8>) -> usize {
    let Some(&c) = bytes.get(i) else {
        out.push(b'\\');
        return i;
    };
    let simple = match c {
        b'n' => Some(b'\n'),
        b'r' => Some(b'\r'),
        b't' => Some(b'\t'),
        b'a' => Some(7),
        b'b' => Some(8),
        b'f' => Some(12),
        b'v' => Some(11),
        b'e' => Some(27),
        b'\\' | b'"' | b'\'' => Some(c),
        _ => None,
    };
    if let Some(b) = simple {
        out.push(b);
        return i + 1;
    }
    let (radix, from, max) = match c {
        b'x' => (16, i + 1, 2),
        // `\0` may be followed by up to three more digits, `\NNN` is three in all.
        b'0' => (8, i + 1, 3),
        b'1'..=b'7' => (8, i, 3),
        _ => {
            out.extend_from_slice(&[b'\\', c]);
            return i + 1;
        }
    };
    let digits = bytes[from..]
        .iter()
        .take(max)
        .take_while(|b| (**b as char).is_digit(radix))
        .count();
    if digits == 0 {
        if c == b'0' {
            out.push(0);
        } else {
            out.extend_from_slice(&[b'\\', c]);
        }
        return i + 1;
    }
    let text = std::str::from_utf8(&bytes[from..from + digits]).unwrap_or("0");
    out.push(u8::from_str_radix(text, radix).unwrap_or(0));
    from + digits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pf(args: &[&str]) -> String {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        String::from_utf8_lossy(&printf(&args).unwrap()).into_owned()
    }

    #[test]
    fn escapes_in_the_format() {
        assert_eq!(pf(&["a\\nb\\rc\\td"]), "a\nb\rc\td");
        assert_eq!(pf(&["\\033[?2004h\\e[0m"]), "\x1b[?2004h\x1b[0m");
        assert_eq!(pf(&["\\033]133;D;0\\007\\a"]), "\x1b]133;D;0\x07\x07");
        assert_eq!(pf(&["\\0033x\\x41\\\\"]), "\x1bxA\\");
        assert_eq!(pf(&["\\q\\"]), "\\q\\");
        assert_eq!(printf(&["\\342\\200\\224".into()]).unwrap(), "\u{2014}".as_bytes());
    }

    #[test]
    fn conversions_pad_and_reuse_the_format() {
        assert_eq!(pf(&["%s|%d|%%", "x", "7"]), "x|7|%");
        assert_eq!(pf(&["%059d", "0"]).len(), 59);
        assert_eq!(pf(&["%05d|%-4s|%4s", "-42", "ab", "cd"]), "-0042|ab  |  cd");
        assert_eq!(pf(&["%s\\n", "a", "b"]), "a\nb\n");
        assert_eq!(pf(&["ONE_%s\\n", "TWO"]), "ONE_TWO\n");
        assert_eq!(pf(&["%s-%s", "only"]), "only-");
        assert_eq!(pf(&["no args", "ignored"]), "no args");
        assert_eq!(pf(&["see %s done\\n", "/usr/lib/x_y"]), "see /usr/lib/x_y done\n");
    }

    #[test]
    fn arguments_are_not_unescaped_and_errors_are_reported() {
        assert_eq!(pf(&["%s", "a\\nb"]), "a\\nb");
        assert!(printf(&[]).is_err());
        assert!(printf(&["%d".into(), "x".into()]).is_err());
        assert!(printf(&["%q".into()]).is_err());
        assert!(printf(&["%".into()]).is_err());
    }
}
