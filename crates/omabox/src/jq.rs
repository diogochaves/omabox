//! jq, run as bin/omabox runs it, for the programs whose answer is jq's own (the desktop's windows,
//! layers and changes: WIN_JQ, DESK_FACTS_JQ, ...): the same program text, the same arguments, so the
//! same bytes out, errors and status included. What a command reads with a plain path stays in
//! json.rs.

use crate::exec::{feed, read_all};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::process::Stdio;

/// Where jq's stderr goes: this process's (the caller's 2>), nowhere, or into the output (`2>&1`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Err {
    Inherit,
    Null,
    Out,
}

/// What a jq run gave: its status (127 when there was no jq to run) and its stdout.
pub struct Ran {
    pub rc: i32,
    pub out: Vec<u8>,
}

impl Ran {
    /// `$(jq ...)`: the output without its trailing newlines.
    pub fn text(&self) -> Vec<u8> {
        crate::shell::strip_nl(&self.out)
    }

    pub fn ok(&self) -> bool {
        self.rc == 0
    }
}

/// `jq ARGS... <<<"$INPUT"` (INPUT None: no stdin, for -n).
pub fn jq<A: AsRef<[u8]>>(args: &[A], input: Option<&[u8]>, err: Err) -> Ran {
    let input = input.map(|i| crate::cat!(i, "\n"));
    tool("jq", args, input.as_deref(), err)
}

/// `awk ARGS... <<<"$INPUT"`: awk as bash runs it, its program verbatim (gawk's numbers are its
/// own); INPUT None: no stdin.
pub fn awk<A: AsRef<[u8]>>(args: &[A], input: Option<&[u8]>, err: Err) -> Ran {
    let input = input.map(|i| crate::cat!(i, "\n"));
    tool("awk", args, input.as_deref(), err)
}

/// `NAME ARGS...` fed INPUT exactly as given (a pipe's bytes; None: /dev/null).
pub fn tool<A: AsRef<[u8]>>(name: &str, args: &[A], input: Option<&[u8]>, err: Err) -> Ran {
    let mut c = crate::sig::command(name);
    c.args(args.iter().map(|a| OsStr::from_bytes(a.as_ref())));
    c.stdin(if input.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let Ok((r, w)) = crate::exec::pipe() else {
        return Ran {
            rc: 1,
            out: Vec::new(),
        };
    };
    let w2 = if err == Err::Out {
        w.try_clone().ok()
    } else {
        None
    };
    c.stdout(Stdio::from(w));
    match (err, w2) {
        (Err::Out, Some(w2)) => c.stderr(Stdio::from(w2)),
        (Err::Null, _) | (Err::Out, None) => c.stderr(Stdio::null()),
        (Err::Inherit, _) => c.stderr(Stdio::inherit()),
    };
    // (a foreground child, sig::run_child)
    let ran = crate::sig::run_child(c, false, |child| {
        let feeder = match (input, child.stdin.take()) {
            (Some(i), Some(si)) => Some(feed(si.into(), i.to_vec())),
            _ => None,
        };
        let out = read_all(r, None);
        if let Some(f) = feeder {
            let _ = f.join();
        }
        out
    });
    match ran {
        Ok((out, (rc, _))) => Ran { rc, out },
        Err(e) => {
            // bash's own line for a command it could not run (in the child, after its
            // redirections: where jq's stderr goes), less its "SCRIPT: line N: ".
            let (rc, m) = not_run(name, &e);
            match err {
                Err::Inherit => {
                    let _ = std::io::Write::write_all(&mut std::io::stderr(), &m);
                }
                Err::Out => return Ran { rc, out: m },
                Err::Null => {}
            }
            Ran {
                rc,
                out: Vec::new(),
            }
        }
    }
}

/// A tool not run: 127 when none is on PATH, else 126 (an argument past 128 KiB: "Argument list too
/// long"), bash's statuses, said in omabox's words (bash's were its own, "jq: command not found":
/// finding 441).
fn not_run(name: &str, e: &std::io::Error) -> (i32, Vec<u8>) {
    let found = crate::shell::env_or_empty("PATH")
        .split(|&c| c == b':')
        .map(|d| crate::cat!(if d.is_empty() { b".".as_slice() } else { d }, "/", name))
        .find(|p| crate::exec::executable(p) && crate::shell::path(p).is_file());
    match (found, e.raw_os_error()) {
        (None, _) | (_, Some(libc::ENOENT)) => (
            127,
            crate::cat!("omabox: cannot run ", name, ": not found in PATH\n"),
        ),
        (Some(p), _) => (
            126,
            crate::cat!("omabox: ", crate::exec::cant_run(&p, e).0, "\n"),
        ),
    }
}

/// `jq -c -n --argjson K V ... PROGRAM` and the like: the words of a call, built in order.
#[derive(Default)]
pub struct Args(Vec<Vec<u8>>);

impl Args {
    pub fn new(first: &[&str]) -> Args {
        Args(first.iter().map(|a| a.as_bytes().to_vec()).collect())
    }

    pub fn arg(mut self, a: impl AsRef<[u8]>) -> Args {
        self.0.push(a.as_ref().to_vec());
        self
    }

    pub fn kv(self, flag: &str, k: &str, v: impl AsRef<[u8]>) -> Args {
        self.arg(flag).arg(k).arg(v)
    }

    pub fn run(&self, input: Option<&[u8]>, err: Err) -> Ran {
        jq(&self.0, input, err)
    }

    /// The words as built.
    pub fn into_words(self) -> Vec<Vec<u8>> {
        self.0
    }

    /// jq reading the file its last word names (no here-string).
    pub fn run_file(&self, err: Err) -> Ran {
        jq(&self.0, None, err)
    }

    /// The same words given to awk (`awk -v K=V ... PROGRAM <<<"$INPUT"`).
    pub fn awk(&self, input: Option<&[u8]>, err: Err) -> Ran {
        awk(&self.0, input, err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `jq -n --argjson c "$BIG"` (one argument past 128 KiB), a jq not on PATH: bash's codes.
    #[test]
    fn not_run_said() {
        let big = vec![b'1'; 200_000];
        let r = Args::new(&["-n"])
            .kv("--argjson", "c", &big)
            .arg("1")
            .run(None, Err::Out);
        assert_eq!(r.rc, 126);
        assert!(r.out.starts_with(b"omabox: cannot run /"));
        assert!(r.out.ends_with(b"/jq: Argument list too long\n"));
        let e = std::io::Error::from_raw_os_error(libc::ENOENT);
        assert_eq!(
            not_run("jq", &e),
            (127, b"omabox: cannot run jq: not found in PATH\n".to_vec())
        );
    }
}
