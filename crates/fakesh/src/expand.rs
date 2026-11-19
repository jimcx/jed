//! Word splitting and expansion: `;`-separated commands, `'…'` / `"…"` quoting, `$VAR`,
//! `${VAR}`, `${VAR+word}`, `${VAR-word}`, `$?`, `$$` and `$((a+b))`. Expanded text is never
//! split or globbed again.

use std::collections::HashMap;
use std::iter::Peekable;
use std::str::Chars;

/// The variables a command sees.
#[derive(Default)]
pub struct Vars {
    map: HashMap<String, String>,
    /// `$?`.
    pub status: i32,
    /// `$$`.
    pub pid: u32,
}

impl Vars {
    /// The process's environment, `$$` from the OS.
    pub fn from_env() -> Vars {
        Vars {
            map: std::env::vars().collect(),
            status: 0,
            pid: std::process::id(),
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.map.get(name).map(String::as_str)
    }

    pub fn set(&mut self, name: &str, value: &str) {
        self.map.insert(name.into(), value.into());
    }

    fn lookup(&self, name: &str) -> Option<String> {
        match name {
            "?" => Some(self.status.to_string()),
            "$" => Some(self.pid.to_string()),
            _ => self.get(name).map(str::to_string),
        }
    }
}

/// `line` cut at its unquoted `;`s (each piece is split into words only when it runs, so a `cd`
/// is seen by the `$PWD` after it).
pub fn split_commands(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quote) = (0, None);
    let mut chars = line.char_indices();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (None, '\\') | (Some('"'), '\\') => {
                chars.next();
            }
            (None, '\'' | '"') => quote = Some(c),
            (Some(q), _) if c == q => quote = None,
            (None, ';') => {
                parts.push(&line[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&line[start..]);
    parts
}

/// The words of one command, quotes removed and `$…` expanded.
pub fn words(cmd: &str, vars: &Vars) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ' ' | '\t' => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
                continue;
            }
            '\'' => {
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    cur.push(c);
                }
            }
            '"' => double_quoted(&mut chars, vars, &mut cur)?,
            '\\' => cur.extend(chars.next()),
            '$' => dollar(&mut chars, vars, &mut cur)?,
            _ => cur.push(c),
        }
        in_word = true;
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

fn double_quoted(chars: &mut Peekable<Chars>, vars: &Vars, out: &mut String) -> Result<(), String> {
    while let Some(c) = chars.next() {
        match c {
            '"' => return Ok(()),
            '$' => dollar(chars, vars, out)?,
            '\\' => match chars.peek() {
                Some(&n @ ('"' | '\\' | '$')) => {
                    out.push(n);
                    chars.next();
                }
                _ => out.push('\\'),
            },
            _ => out.push(c),
        }
    }
    Err("unterminated quote".into())
}

/// Expands what follows a `$` (already consumed) into `out`.
fn dollar(chars: &mut Peekable<Chars>, vars: &Vars, out: &mut String) -> Result<(), String> {
    match chars.peek().copied() {
        Some('(') => {
            chars.next();
            if chars.next() != Some('(') {
                return Err("command substitution is not supported".into());
            }
            let (mut expr, mut depth) = (String::new(), 0);
            loop {
                match chars.next() {
                    None => return Err("unterminated $((".into()),
                    Some(')') if depth == 0 => {
                        if chars.next() != Some(')') {
                            return Err("bad arithmetic".into());
                        }
                        break;
                    }
                    Some(c) => {
                        match c {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            _ => {}
                        }
                        expr.push(c);
                    }
                }
            }
            out.push_str(&arith(&expr, vars)?.to_string());
        }
        Some('{') => {
            chars.next();
            let mut body = String::new();
            loop {
                match chars.next() {
                    None => return Err("unterminated ${".into()),
                    Some('}') => break,
                    Some(c) => body.push(c),
                }
            }
            let name_len = body
                .find(|c: char| !(c.is_alphanumeric() || c == '_'))
                .unwrap_or(body.len());
            let (name, op) = body.split_at(name_len);
            let value = vars.lookup(name);
            let op = op.trim_start_matches(':');
            let word = op.get(1..).unwrap_or("");
            match op.chars().next() {
                None => out.push_str(value.as_deref().unwrap_or("")),
                Some('+') => out.push_str(if value.is_some() { word } else { "" }),
                Some('-') => out.push_str(value.as_deref().unwrap_or(word)),
                Some(_) => return Err(format!("bad substitution: ${{{body}}}")),
            }
        }
        Some(c @ ('?' | '$')) => {
            chars.next();
            out.push_str(&vars.lookup(&c.to_string()).unwrap_or_default());
        }
        Some(c) if c.is_alphabetic() || c == '_' => {
            let mut name = String::new();
            while let Some(&c) = chars.peek().filter(|c| c.is_alphanumeric() || **c == '_') {
                name.push(c);
                chars.next();
            }
            out.push_str(&vars.lookup(&name).unwrap_or_default());
        }
        _ => out.push('$'),
    }
    Ok(())
}

/// `a + b * (c - 1)` over integers and variables (`+ - * / %`, unary signs, parentheses).
pub fn arith(expr: &str, vars: &Vars) -> Result<i64, String> {
    let chars: Vec<char> = expr.chars().filter(|c| !c.is_whitespace()).collect();
    let mut p = Arith {
        chars: &chars,
        at: 0,
        vars,
    };
    let value = p.sum()?;
    match chars.get(p.at) {
        None => Ok(value),
        Some(c) => Err(format!("arithmetic: unexpected `{c}` in `{expr}`")),
    }
}

struct Arith<'a> {
    chars: &'a [char],
    at: usize,
    vars: &'a Vars,
}

impl Arith<'_> {
    fn eat(&mut self, ops: &[char]) -> Option<char> {
        let c = *self.chars.get(self.at).filter(|c| ops.contains(c))?;
        self.at += 1;
        Some(c)
    }

    fn sum(&mut self) -> Result<i64, String> {
        let mut v = self.product()?;
        while let Some(op) = self.eat(&['+', '-']) {
            let r = self.product()?;
            v = if op == '+' {
                v.wrapping_add(r)
            } else {
                v.wrapping_sub(r)
            };
        }
        Ok(v)
    }

    fn product(&mut self) -> Result<i64, String> {
        let mut v = self.unary()?;
        while let Some(op) = self.eat(&['*', '/', '%']) {
            let r = self.unary()?;
            v = match op {
                '*' => v.wrapping_mul(r),
                _ if r == 0 => return Err("division by 0".into()),
                '/' => v.wrapping_div(r),
                _ => v.wrapping_rem(r),
            };
        }
        Ok(v)
    }

    fn unary(&mut self) -> Result<i64, String> {
        if let Some(sign) = self.eat(&['+', '-']) {
            let v = self.unary()?;
            return Ok(if sign == '-' { v.wrapping_neg() } else { v });
        }
        if self.eat(&['(']).is_some() {
            let v = self.sum()?;
            return self
                .eat(&[')'])
                .map(|_| v)
                .ok_or_else(|| "arithmetic: missing `)`".into());
        }
        let start = self.at;
        while self
            .chars
            .get(self.at)
            .is_some_and(|c| c.is_alphanumeric() || *c == '_')
        {
            self.at += 1;
        }
        let token: String = self.chars[start..self.at].iter().collect();
        if token.starts_with(|c: char| c.is_ascii_digit()) {
            token.parse().map_err(|_| format!("arithmetic: bad number `{token}`"))
        } else if token.is_empty() {
            Err("arithmetic: operand expected".into())
        } else {
            Ok(self.vars.get(&token).and_then(|v| v.trim().parse().ok()).unwrap_or(0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        let mut v = Vars::default();
        v.set("A", "alpha");
        v.set("N", "6");
        v.set("EMPTY", "");
        v.status = 127;
        v.pid = 42;
        v
    }

    fn w(cmd: &str) -> Vec<String> {
        words(cmd, &vars()).unwrap()
    }

    #[test]
    fn commands_split_at_unquoted_semicolons() {
        assert_eq!(split_commands("a; b ;c"), ["a", " b ", "c"]);
        assert_eq!(
            split_commands("echo 'a;b' \"c;d\" e\\;f; g"),
            ["echo 'a;b' \"c;d\" e\\;f", " g"]
        );
        assert_eq!(split_commands(""), [""]);
    }

    #[test]
    fn words_split_on_spaces_and_honour_quotes() {
        assert_eq!(w("  echo  a   b "), ["echo", "a", "b"]);
        assert_eq!(w("echo 'a  b' \"c  d\""), ["echo", "a  b", "c  d"]);
        assert_eq!(w("echo RE''ADY"), ["echo", "READY"]);
        assert_eq!(w("echo '' x"), ["echo", "", "x"]);
        assert_eq!(w(r#"echo "q\"d\$A" a\ b"#), ["echo", "q\"d$A", "a b"]);
        assert_eq!(w("echo 'no $A'"), ["echo", "no $A"]);
        assert!(words("echo \"open", &vars()).is_err());
    }

    #[test]
    fn variables_expand_bare_braced_and_quoted() {
        assert_eq!(w("$A ${A}x \"$A\" x$N.y"), ["alpha", "alphax", "alpha", "x6.y"]);
        assert_eq!(w("$MISSING-x"), ["-x"]);
        assert_eq!(w("$? $$"), ["127", "42"]);
        assert_eq!(w("a$ $"), ["a$", "$"]);
    }

    #[test]
    fn default_and_alternative_forms() {
        assert_eq!(
            w("${A+set} ${MISSING+set}x ${MISSING-unset} ${A-unset} ${EMPTY+set}"),
            ["set", "x", "unset", "alpha", "set"]
        );
        assert!(words("${A", &vars()).is_err());
        assert!(words("${A#x}", &vars()).is_err());
    }

    #[test]
    fn arithmetic_has_precedence_parentheses_and_variables() {
        let v = vars();
        for (expr, want) in [
            ("1+1", 2),
            ("2+3*4", 14),
            ("(2+3)*4", 20),
            (" 100 + 2 ", 102),
            ("N*N", 36),
            ("-3+5", 2),
            ("7/2", 3),
            ("7%4", 3),
            ("MISSING+1", 1),
            ("10-2-3", 5),
        ] {
            assert_eq!(arith(expr, &v), Ok(want), "{expr}");
        }
        assert!(arith("1/0", &v).is_err());
        assert!(arith("1+", &v).is_err());
        assert!(arith("(1", &v).is_err());
        assert!(arith("1+*2", &v).is_err());
        assert_eq!(w("TAG$((100+2)) $((N*(1+1)))"), ["TAG102", "12"]);
    }
}
