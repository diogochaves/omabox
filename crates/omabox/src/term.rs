//! Text a box wrote, fit for the caller's terminal (S7): an escape sequence the box wrote never
//! reaches the terminal as one. Every place a box's words (a plugin's, a tray item's, an agent's, a
//! helper's answer in the box) are said in plain text goes through here; `--json` is escaped by serde.

/// What `plain` keeps of a box's text by default: a name, a label, a line.
pub const CAP: usize = 400;

/// V fit for a terminal: a control character (but newline and tab) or a byte that is no UTF-8 as
/// `?`, at most CAP bytes of it, what is cut said.
pub fn plain_cap(v: &[u8], cap: usize) -> Vec<u8> {
    let (head, cut) = if v.len() > cap {
        (&v[..cap], v.len() - cap)
    } else {
        (v, 0)
    };
    let mut out = Vec::with_capacity(head.len());
    for chunk in head.utf8_chunks() {
        for c in chunk.valid().chars() {
            if c.is_control() && c != '\n' && c != '\t' {
                out.push(b'?');
            } else {
                let mut b = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
            }
        }
        out.extend(std::iter::repeat_n(b'?', chunk.invalid().len()));
    }
    if cut > 0 {
        out.extend_from_slice(format!("... ({cut} bytes more)").as_bytes());
    }
    out
}

/// `plain_cap` at CAP.
pub fn plain(v: &[u8]) -> Vec<u8> {
    plain_cap(v, CAP)
}

/// `plain` of a string, as one (a name or a label in a line of text).
pub fn plain_str(s: &str) -> String {
    String::from_utf8_lossy(&plain(s.as_bytes())).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_keeps_escapes_off_the_terminal() {
        assert_eq!(plain(b"/a/b\tc\nd"), b"/a/b\tc\nd");
        assert_eq!(plain(b"x\x1b]52;c;aGk=\x07y"), b"x?]52;c;aGk=?y");
        assert_eq!(plain(b"\x1b[31mred\r"), b"?[31mred?");
        assert_eq!(plain("a\u{9b}b é".as_bytes()), "a?b é".as_bytes());
        assert_eq!(plain(b"a\x9bb\xff"), b"a?b?");
        assert_eq!(plain(b"del\x7f"), b"del?");
        let long = vec![b'x'; 1000];
        let p = plain(&long);
        assert!(p.starts_with(&[b'x'; CAP]));
        assert!(p.ends_with(b"... (600 bytes more)"));
        assert_eq!(plain_cap(b"abcdef", 3), b"abc... (3 bytes more)");
        // A character cut at the cap is no UTF-8: a `?`, never half a sequence.
        assert_eq!(plain_cap("aé".as_bytes(), 2), b"a?... (1 bytes more)");
        assert_eq!(plain_str("id\x1b[2Jx"), "id?[2Jx");
    }
}
