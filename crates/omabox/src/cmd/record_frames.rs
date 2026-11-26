//! `record --frames-log FILE` (#488, finding 763): which captured frame each frame of the video
//! holds. wf-recorder's raw stream has every frame it captured at the compositor's own time; the
//! video is that stream through ffmpeg's `fps` filter, which repeats a frame where none came in a
//! video frame's time and leaves frames out where several did. Which, ffmpeg itself says: a second
//! pass over the raw stream with each frame's pixels replaced by its number and its time (bits, a
//! pixel each, by `geq`) through the same `fps` filter gives one tiny frame per video frame, read
//! back here. No rule of the filter's is guessed at.
//!
//! What it cannot say: a frame the compositor drew between two of the recorder's captures was
//! never in the raw stream. The captured frames' times are in the log, so such a gap shows.

/// The stamped frame: BITS pixels a row, 4 rows (two of the number, two of the time in ms), gray.
const BITS: usize = 32;
pub const STAMP_BYTES: usize = BITS * 4;
/// What the second pass may write: 16 MiB, 131072 video frames (36 minutes at 60 fps).
pub const STAMP_MAX: usize = 16_777_216;

/// The second pass's filter: BACK (the turn the video's own filter starts with: it changes no
/// time), the stamp, then the video's `fps`.
pub fn stamp_filter(back: &str, fps: &[u8]) -> Vec<u8> {
    crate::cat!(
        back,
        "scale=32:4:flags=neighbor,format=gray,",
        "geq=lum='if(lt(Y,2),255*mod(floor(N/pow(2,X)),2),255*mod(floor(round(T*1000)/pow(2,X)),2))',",
        "fps=",
        fps
    )
}

/// The stamps read back: each video frame's raw frame (from 0) and that frame's time in the raw
/// stream (ms). None when a stamp is not one (a pixel neither black nor white, rows that differ).
pub fn stamps(raw: &[u8]) -> Option<Vec<(u64, u64)>> {
    let mut v = Vec::with_capacity(raw.len() / STAMP_BYTES);
    for f in raw.chunks_exact(STAMP_BYTES) {
        let word = |row: usize| -> Option<u64> {
            let r = &f[row * BITS..(row + 1) * BITS];
            if r != &f[(row + 1) * BITS..(row + 2) * BITS] {
                return None;
            }
            r.iter().enumerate().try_fold(0u64, |a, (x, &p)| match p {
                0..=15 => Some(a),
                240..=255 => Some(a | 1 << x),
                _ => None,
            })
        };
        v.push((word(0)?, word(2)?));
    }
    Some(v)
}

/// The log's counts: the video's frames, those that hold a frame not shown before, those that
/// repeat the one before, and the captured frames no video frame holds.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Counts {
    pub frames: usize,
    pub new: usize,
    pub repeated: usize,
    pub skipped: u64,
}

/// The log (a TSV with a header) and its counts, of STAMPS at FPS. None when the stamps are not a
/// video's: empty, or a frame number going back.
pub fn log(stamps: &[(u64, u64)], fps: f64) -> Option<(Vec<u8>, Counts)> {
    let &(first, t0) = stamps.first()?;
    let mut c = Counts {
        frames: stamps.len(),
        new: 0,
        repeated: 0,
        skipped: first,
    };
    let mut o = b"frame\tms\tholds\tdrawn_ms\twhat\tskipped\n".to_vec();
    let mut prev: Option<u64> = None;
    for (k, &(n, t)) in stamps.iter().enumerate() {
        let (what, skipped) = match prev {
            Some(p) if n == p => ("repeated", 0),
            Some(p) if n < p => return None,
            Some(p) => ("new", n - p - 1),
            None => ("new", n),
        };
        if what == "new" {
            c.new += 1;
        } else {
            c.repeated += 1;
        }
        if prev.is_some() {
            c.skipped += skipped;
        }
        prev = Some(n);
        o.extend_from_slice(
            format!(
                "{}\t{:.3}\t{}\t{}\t{what}\t{skipped}\n",
                k + 1,
                k as f64 * 1000.0 / fps,
                n + 1,
                t.saturating_sub(t0)
            )
            .as_bytes(),
        );
    }
    Some((o, c))
}

/// The summary line.
pub fn summary(c: Counts) -> String {
    format!(
        "{} frames: {} new, {} repeated, {} skipped",
        c.frames, c.new, c.repeated, c.skipped
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stamp as ffmpeg writes it: N and T, a pixel a bit, each row twice.
    fn stamp(n: u64, t: u64) -> Vec<u8> {
        let row = |v: u64| -> Vec<u8> {
            (0..BITS)
                .map(|x| if v >> x & 1 == 1 { 255 } else { 0 })
                .collect()
        };
        [row(n), row(n), row(t), row(t)].concat()
    }

    #[test]
    fn stamps_read_back() {
        let raw = [stamp(0, 0), stamp(2, 33), stamp(70_000, 1_166_650)].concat();
        assert_eq!(
            stamps(&raw),
            Some(vec![(0, 0), (2, 33), (70_000, 1_166_650)])
        );
        // (a cut-off last stamp is left out: what a capture's own newline adds)
        assert_eq!(
            stamps(&[stamp(5, 9), vec![10]].concat()),
            Some(vec![(5, 9)])
        );
        assert_eq!(stamps(b""), Some(vec![]));
        // a gray pixel, or two rows that differ: not a stamp
        let mut bad = stamp(1, 1);
        bad[3] = 128;
        bad[BITS + 3] = 128;
        assert_eq!(stamps(&bad), None);
        let mut bad = stamp(1, 1);
        bad[0] = 0;
        assert_eq!(stamps(&bad), None);
    }

    #[test]
    fn log_says_new_repeated_and_skipped() {
        // 30 fps of a 60 Hz capture with one frame late: 0, 2, (nothing new) 2, 5, 6
        let s = [(0, 100), (2, 133), (2, 133), (5, 183), (6, 200)];
        let (text, c) = log(&s, 30.0).unwrap();
        assert_eq!(
            String::from_utf8(text).unwrap(),
            "frame\tms\tholds\tdrawn_ms\twhat\tskipped\n\
             1\t0.000\t1\t0\tnew\t0\n\
             2\t33.333\t3\t33\tnew\t1\n\
             3\t66.667\t3\t33\trepeated\t0\n\
             4\t100.000\t6\t83\tnew\t2\n\
             5\t133.333\t7\t100\tnew\t0\n"
        );
        assert_eq!(
            c,
            Counts {
                frames: 5,
                new: 4,
                repeated: 1,
                skipped: 3
            }
        );
        assert_eq!(summary(c), "5 frames: 4 new, 1 repeated, 3 skipped");
    }

    #[test]
    fn log_refuses_what_is_no_video() {
        assert_eq!(log(&[], 30.0), None);
        assert_eq!(log(&[(3, 0), (2, 5)], 30.0), None);
        // a first video frame that is not the first captured: those before it were skipped
        let (_, c) = log(&[(2, 40), (3, 57)], 60.0).unwrap();
        assert_eq!((c.new, c.skipped), (2, 2));
    }

    #[test]
    fn filter_words() {
        assert_eq!(
            String::from_utf8(stamp_filter("transpose=2,", b"60")).unwrap(),
            "transpose=2,scale=32:4:flags=neighbor,format=gray,geq=lum='if(lt(Y,2),255*mod(floor(N/pow(2,X)),2),255*mod(floor(round(T*1000)/pow(2,X)),2))',fps=60"
        );
    }
}
