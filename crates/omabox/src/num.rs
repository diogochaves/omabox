//! Numbers as bin/omabox writes and reads them: the one duration grammar (ms_duration, #273), awk's
//! printf of seconds (ms_s), and awk's reading and writing of a number in a field.

use crate::cat;
use crate::exit::{Ctx, Res};
use crate::shell::Re;

/// V as a number (shell::dec), or the command ends saying WHAT it could not read, 1 (finding 448:
/// where bash's arithmetic ended it in its own words, or wrapped).
pub fn read(ctx: &Ctx, what: &str, v: &[u8]) -> Res<i64> {
    crate::shell::dec(v).ok_or_else(|| {
        ctx.die(cat!(
            ctx.label(),
            "cannot read ",
            what,
            " as a number: '",
            v,
            "'"
        ))
    })
}

/// ms_duration: the one duration grammar (#273): 300ms, 2s, 1.5s, 2m, 1h, or a bare number of
/// seconds, as milliseconds (at least 1).
pub fn ms_duration(v: &[u8]) -> Option<i64> {
    let re = Re::new(r"^([0-9]{1,6}(\.[0-9]{1,6})?)(ms|s|m|h)?$")?;
    let n: f64 = String::from_utf8_lossy(&re.group(v, 1)?).parse().ok()?;
    let unit = re.group(v, 3).unwrap_or_default();
    let mult = match unit.as_slice() {
        b"ms" => 1.0,
        b"h" => 3_600_000.0,
        b"m" => 60_000.0,
        _ => 1000.0,
    };
    let ms = (n * mult + 0.5).trunc();
    (ms >= 1.0).then_some(ms as i64)
}

/// ms_position: a time in a video or a log, ms_duration's grammar with zero let in (0, 0s, 0ms,
/// 0.0: the start; #492).
pub fn ms_position(v: &[u8]) -> Option<i64> {
    if crate::shell::re_match(r"^0+(\.0+)?(ms|s|m|h)?$", v) {
        return Some(0);
    }
    ms_duration(v)
}

/// s_duration V [zero]: a duration (ms_duration's) as seconds, decimals only where it has them
/// (1.5s, 1500ms: 1.5; 5: 5); ZERO lets 0 in, in any unit.
pub fn s_duration(v: &[u8], zero: bool) -> Option<String> {
    if zero && crate::shell::re_match(r"^0+(\.0+)?(ms|s|m|h)?$", v) {
        return Some("0".to_string());
    }
    let ms = ms_duration(v)?;
    // (awk's `sprintf("%.3f", m / 1000)` and `sub(/\.?0+$/, "")`)
    let mut s = fixed(ms as f64 / 1000.0, 3);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    Some(s)
}

/// `printf "%.Nf"` of X, as awk and printf(1) write it (glibc's rounding of the exact value).
pub fn fixed(x: f64, prec: usize) -> String {
    let fmt = format!("%.{prec}f\0");
    let mut buf = [0u8; 512];
    // SAFETY: buf has its length given; fmt is NUL-ended and takes one double.
    let n = unsafe { libc::snprintf(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr().cast(), x) };
    let n = usize::try_from(n).unwrap_or(0).min(buf.len() - 1);
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// ms_s MS: milliseconds as seconds, 2 decimals (`awk 'BEGIN { printf "%.2fs", m / 1000 }'`): 430 →
/// 0.43s.
pub fn ms_s(ms: impl AsRef<[u8]>) -> String {
    format!("{}s", fixed(awk_num(ms.as_ref()) / 1000.0, 2))
}

/// awk's value of a string as a number: its leading number (blanks first: space, \t, \n, \v, \f,
/// \r), 0 for none; gawk's +inf, -inf, +nan, -nan as such (exactly those, blanks around).
pub fn awk_num(v: &[u8]) -> f64 {
    let s = String::from_utf8_lossy(v);
    let blank = [' ', '\t', '\n', '\u{b}', '\u{c}', '\r'];
    let t = s.trim_start_matches(blank);
    let whole = t.trim_end_matches(blank).to_ascii_lowercase();
    match whole.as_str() {
        "+inf" => return f64::INFINITY,
        "-inf" => return f64::NEG_INFINITY,
        "+nan" => return f64::NAN,
        "-nan" => return -f64::NAN,
        _ => {}
    }
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let ds = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    if i == ds || (i == ds + 1 && b[ds] == b'.') {
        return 0.0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    t[..i].parse().unwrap_or(0.0)
}

/// A number as gawk writes it with %s or print: an integral value whole, every digit (%.0f, -0 as
/// 0), +inf, -inf, +nan, -nan as such, anything else CONVFMT's %.6g.
pub fn awk_str(x: f64) -> String {
    if x.is_nan() {
        return if x.is_sign_negative() { "-nan" } else { "+nan" }.to_string();
    }
    if x.is_infinite() {
        return if x > 0.0 { "+inf" } else { "-inf" }.to_string();
    }
    if x == x.trunc() {
        return if x == 0.0 {
            "0".to_string()
        } else {
            fixed(x, 0)
        };
    }
    let mut buf = [0u8; 64];
    // SAFETY: buf has its length given; the format is NUL-ended and takes one double.
    let n = unsafe { libc::snprintf(buf.as_mut_ptr().cast(), buf.len(), c"%.6g".as_ptr(), x) };
    let n = usize::try_from(n).unwrap_or(0).min(buf.len() - 1);
    String::from_utf8_lossy(&buf[..n]).into_owned()
}

/// `awk -F, '{ printf "%s,%s %sx%s", $1, $2, $3, $4 }' <<<"$V"`: X,Y,W,H as "X,Y WxH" (the fields as
/// they are, missing ones empty), of V's first line.
pub fn xywh_words(v: &[u8]) -> Vec<u8> {
    let line = v.split(|&c| c == b'\n').next().unwrap_or_default();
    let f: Vec<&[u8]> = line.split(|&c| c == b',').collect();
    let g = |i: usize| f.get(i).copied().unwrap_or_default();
    crate::cat!(g(0), ",", g(1), " ", g(2), "x", g(3))
}

/// The same with the place moved back by OX OY (`$1 - ox, $2 - oy`: awk's numbers).
pub fn xywh_less(v: &[u8], ox: &[u8], oy: &[u8]) -> Vec<u8> {
    let line = v.split(|&c| c == b'\n').next().unwrap_or_default();
    let f: Vec<&[u8]> = line.split(|&c| c == b',').collect();
    let g = |i: usize| f.get(i).copied().unwrap_or_default();
    crate::cat!(
        awk_str(awk_num(g(0)) - awk_num(ox)),
        ",",
        awk_str(awk_num(g(1)) - awk_num(oy)),
        " ",
        g(2),
        "x",
        g(3)
    )
}

/// A value as `awk -v NAME=VALUE` makes it: its escapes (\\n, \\t, \\\\, octal...) read as in an awk
/// string.
pub fn awk_v(v: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len());
    let mut i = 0;
    while i < v.len() {
        if v[i] != b'\\' || i + 1 == v.len() {
            out.push(v[i]);
            i += 1;
            continue;
        }
        i += 1;
        let c = v[i];
        i += 1;
        match c {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'a' => out.push(7),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'v' => out.push(11),
            b'0'..=b'7' => {
                let mut n = u32::from(c - b'0');
                let mut k = 1;
                while k < 3 && i < v.len() && (b'0'..=b'7').contains(&v[i]) {
                    n = n * 8 + u32::from(v[i] - b'0');
                    i += 1;
                    k += 1;
                }
                out.push(u8::try_from(n & 0xff).unwrap_or(0));
            }
            // (\\, \", \/ and any other: the character itself)
            c => out.push(c),
        }
    }
    out
}

/// human_duration SECS: seconds as the user would write them: 90s, 30m, 2h.
pub fn human_duration(s: i64) -> String {
    if s > 0 && s % 3600 == 0 {
        format!("{}h", s / 3600)
    } else if s > 0 && s % 60 == 0 {
        format!("{}m", s / 60)
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// t_unit_duration's (one grammar, #273, finding 385), as bash's ms_duration, s_duration and
    /// human_duration read them.
    #[test]
    fn durations_as_bash() {
        let ms: Vec<Option<i64>> = ["300ms", "2", "1.5s", "2m", "1h", "1.5h"]
            .iter()
            .map(|v| ms_duration(v.as_bytes()))
            .collect();
        assert_eq!(
            ms,
            [300, 2000, 1500, 120_000, 3_600_000, 5_400_000].map(Some)
        );
        for bad in ["0", "2d", "-1"] {
            assert_eq!(ms_duration(bad.as_bytes()), None, "{bad}");
        }
        let s: Vec<Option<String>> = ["5", "1.5s", "500ms", "2m"]
            .iter()
            .map(|v| s_duration(v.as_bytes(), false))
            .collect();
        assert_eq!(s, ["5", "1.5", "0.5", "120"].map(|x| Some(x.to_string())));
        assert_eq!(s_duration(b"0", true).as_deref(), Some("0"));
        assert_eq!(s_duration(b"0s", true).as_deref(), Some("0"));
        assert_eq!(s_duration(b"0", false), None);
        assert_eq!(human_duration(90), "90s");
        assert_eq!(human_duration(7200), "2h");
    }

    #[test]
    fn durations() {
        // A position lets zero in, in any unit (#492); a duration does not.
        for z in ["0", "0s", "0ms", "0.0", "00.000s"] {
            assert_eq!(ms_position(z.as_bytes()), Some(0), "{z}");
            assert_eq!(ms_duration(z.as_bytes()), None, "{z}");
        }
        assert_eq!(ms_position(b"0.7"), Some(700));
        assert_eq!(ms_position(b"-0"), None);
        assert_eq!(ms_duration(b"500ms"), Some(500));
        assert_eq!(ms_duration(b"1.5s"), Some(1500));
        assert_eq!(ms_duration(b"1.5"), Some(1500));
        assert_eq!(ms_duration(b"1.5m"), Some(90_000));
        assert_eq!(ms_duration(b"2m"), Some(120_000));
        assert_eq!(ms_duration(b"3"), Some(3000));
        assert_eq!(ms_duration(b"0.0004"), None);
        assert_eq!(s_duration(b"5", false).as_deref(), Some("5"));
        assert_eq!(s_duration(b"500ms", false).as_deref(), Some("0.5"));
        assert_eq!(s_duration(b"10s", false).as_deref(), Some("10"));
        assert_eq!(s_duration(b"1.25s", false).as_deref(), Some("1.25"));
        assert_eq!(s_duration(b"2m", false).as_deref(), Some("120"));
        assert_eq!(s_duration(b"0", false), None);
        assert_eq!(s_duration(b"0ms", true).as_deref(), Some("0"));
        assert_eq!(s_duration(b"00.0s", true).as_deref(), Some("0"));
        assert_eq!(ms_duration(b"0.0005"), Some(1));
        assert_eq!(ms_duration(b"x"), None);
    }

    // What gawk's printf "%.2fs" wrote for each.
    #[test]
    fn seconds_as_awk() {
        assert_eq!(ms_s(b"430"), "0.43s");
        assert_eq!(ms_s(b"125"), "0.12s");
        assert_eq!(ms_s(b"135"), "0.14s");
        assert_eq!(ms_s(b"5"), "0.01s");
        assert_eq!(ms_s(b"0"), "0.00s");
        assert_eq!(ms_s(b""), "0.00s");
        assert_eq!(ms_s(b"12x"), "0.01s");
        assert_eq!(ms_s(b"600000"), "600.00s");
    }

    #[test]
    fn awk_numbers() {
        assert_eq!(awk_num(b" 12abc"), 12.0);
        assert_eq!(awk_num(b"-3.5e2x"), -350.0);
        assert_eq!(awk_num(b"x"), 0.0);
        assert_eq!(awk_num(b"."), 0.0);
        assert_eq!(awk_str(5.0), "5");
        assert_eq!(awk_str(-0.5), "-0.5");
        assert_eq!(awk_str(1.0 / 3.0), "0.333333");
        // (gawk 5.4's words for each)
        assert_eq!(
            awk_str(awk_num(b"-12345678901234567")),
            "-12345678901234568"
        );
        assert_eq!(awk_str(1e20), "100000000000000000000");
        assert_eq!(awk_str(1234567.5), "1.23457e+06");
        assert_eq!(awk_str(-0.0), "0");
        assert_eq!(awk_str(awk_num(b"1e999")), "+inf");
        assert_eq!(awk_str(awk_num(b" -INF ")), "-inf");
        assert_eq!(awk_str(awk_num(b"-nan")), "-nan");
        assert_eq!(awk_num(b"+inf5"), 0.0);
        assert_eq!(awk_num(b"\x0b\x0c\r12"), 12.0);
        assert_eq!(xywh_words(b"1,2,30,40"), b"1,2 30x40");
        assert_eq!(xywh_words(b"1,2"), b"1,2 x");
        assert_eq!(xywh_less(b"110,220,30,40", b"100", b"200"), b"10,20 30x40");
        assert_eq!(awk_v(b"a\\tb\\\\c\\101"), b"a\tb\\cA");
    }
}
