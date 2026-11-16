//! Times on forge comments: the ISO 8601 strings the adapters pass through, and the short
//! relative text a comment's header shows. Pure; the caller supplies the clock.

use crate::git_show::civil_from_days;

/// Days from 1970-01-01 of a civil date (Hinnant's `days_from_civil`, the inverse of
/// `git_show::civil_from_days`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `n` ASCII digits at the start of `s`, and the rest.
fn digits(s: &str, n: usize) -> Option<(i64, &str)> {
    let head = s.get(..n)?;
    if !head.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some((head.parse().ok()?, &s[n..]))
}

/// An ISO 8601 / RFC 3339 time (`2026-03-12T10:20:30Z`, `…30.123Z`, `…30+01:00`, `…30-0500`;
/// no offset means UTC; a space may replace the `T`) as seconds since the Unix epoch. Anything
/// else, an empty string included, gives `None`.
pub fn parse_iso(s: &str) -> Option<i64> {
    let (y, s) = digits(s.trim(), 4)?;
    let (mo, s) = digits(s.strip_prefix('-')?, 2)?;
    let (d, s) = digits(s.strip_prefix('-')?, 2)?;
    let s = s.strip_prefix(['T', 't', ' '])?;
    let (h, s) = digits(s, 2)?;
    let (mi, s) = digits(s.strip_prefix(':')?, 2)?;
    let (sec, mut s) = digits(s.strip_prefix(':')?, 2)?;
    if let Some(frac) = s.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return None;
        }
        s = &frac[n..];
    }
    let offset = match s {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match s.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let (oh, rest) = digits(&s[1..], 2)?;
            let rest = rest.strip_prefix(':').unwrap_or(rest);
            let (om, rest) = digits(rest, 2)?;
            if !rest.is_empty() || oh > 23 || om > 59 {
                return None;
            }
            sign * (oh * 3600 + om * 60)
        }
    };
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + sec - offset)
}

/// Days shown as `3d ago`; from this many days on, a date.
const DAYS: i64 = 7;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// How long before `now` (epoch seconds) `then` was, compactly: `just now` (under a minute, or
/// a time ahead of the clock), `5m ago`, `2h ago`, `3d ago` (under 7 days), then the date in
/// UTC: `12 Mar` in `now`'s year, `12 Mar 2025` in another. (`history::ago` is the longer
/// form the history list and blame use.)
pub fn ago(now: i64, then: i64) -> String {
    let secs = now - then;
    if secs < 60 {
        "just now".into()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else if secs < DAYS * 86_400 {
        format!("{}d ago", secs / 86_400)
    } else {
        let (y, m, d) = civil_from_days(then.div_euclid(86_400));
        let month = MONTHS[(m - 1) as usize];
        if civil_from_days(now.div_euclid(86_400)).0 == y {
            format!("{d} {month}")
        } else {
            format!("{d} {month} {y}")
        }
    }
}

/// Now in epoch seconds (the system clock).
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: i64 = 1_773_310_830; // 2026-03-12T10:20:30Z

    #[test]
    fn parses_the_forms_the_forges_send() {
        assert_eq!(parse_iso("2026-03-12T10:20:30Z"), Some(T));
        assert_eq!(parse_iso("2026-03-12T10:20:30.123Z"), Some(T));
        assert_eq!(parse_iso("2026-03-12T10:20:30.123456789Z"), Some(T));
        assert_eq!(parse_iso("2026-03-12T11:20:30+01:00"), Some(T));
        assert_eq!(parse_iso("2026-03-12T05:20:30-05:00"), Some(T));
        assert_eq!(parse_iso("2026-03-12T05:20:30-0500"), Some(T));
        assert_eq!(parse_iso("2026-03-12T10:20:30"), Some(T));
        assert_eq!(parse_iso("2026-03-12 10:20:30Z"), Some(T));
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_iso("1969-12-31T23:59:59Z"), Some(-1));
        assert_eq!(parse_iso("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    }

    #[test]
    fn garbage_gives_no_time() {
        for s in [
            "",
            "yesterday",
            "2026-03-12",
            "2026-13-12T10:20:30Z",
            "2026-03-12T25:20:30Z",
            "2026-03-12T10:20:30.Z",
            "2026-03-12T10:20:30+1",
            "2026-03-12T10:20:30Zjunk",
            "２026-03-12T10:20:30Z",
        ] {
            assert_eq!(parse_iso(s), None, "{s:?}");
        }
    }

    #[test]
    fn civil_dates_round_trip_and_offsets_move_the_day() {
        for (y, m, d) in [
            (2024, 2, 29),
            (2000, 3, 1),
            (1999, 12, 31),
            (1900, 3, 1),
            (2100, 3, 1),
            (1970, 1, 1),
            (2026, 12, 31),
        ] {
            assert_eq!(civil_from_days(days_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(days_from_civil(2000, 3, 1) - days_from_civil(2000, 2, 28), 2);
        assert_eq!(days_from_civil(2100, 3, 1) - days_from_civil(2100, 2, 28), 1);
        // 23:30 at -05:00 is already the next day (and year) in UTC; and the reverse.
        assert_eq!(
            parse_iso("2025-12-31T23:30:00-05:00"),
            parse_iso("2026-01-01T04:30:00Z")
        );
        assert_eq!(
            parse_iso("2026-01-01T00:30:00+01:00"),
            parse_iso("2025-12-31T23:30:00Z")
        );
        assert_eq!(parse_iso("2000-03-01T00:00:00Z"), Some(951_868_800));
    }

    #[test]
    fn ago_steps() {
        assert_eq!(ago(T, T), "just now");
        assert_eq!(ago(T, T + 500), "just now"); // ahead of the clock
        assert_eq!(ago(T + 59, T), "just now");
        assert_eq!(ago(T + 60, T), "1m ago");
        assert_eq!(ago(T + 3599, T), "59m ago");
        assert_eq!(ago(T + 3600, T), "1h ago");
        assert_eq!(ago(T + 2 * 3600 + 5, T), "2h ago");
        assert_eq!(ago(T + 86_399, T), "23h ago");
        assert_eq!(ago(T + 86_400, T), "1d ago");
        assert_eq!(ago(T + 7 * 86_400 - 1, T), "6d ago");
        assert_eq!(ago(T + 7 * 86_400, T), "12 Mar");
        assert_eq!(ago(T + 6 * 86_400, T), "6d ago");
        assert_eq!(ago(T + 59 * 60, T), "59m ago");
        assert_eq!(ago(T - 1, T), "just now");
        assert_eq!(ago(T + 200 * 86_400, T), "12 Mar");
        // Another year than the clock's.
        assert_eq!(ago(T + 365 * 86_400, T), "12 Mar 2026");
        assert_eq!(ago(parse_iso("2027-01-02T00:00:00Z").unwrap(), T), "12 Mar 2026");
    }
}
