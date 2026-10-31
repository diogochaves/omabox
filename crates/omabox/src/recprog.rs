//! record's programs that run as bash's omabox had them inline (#324 wave 4b): the shell its box
//! runs, awk's and jq's. Taken out of bash's omabox byte for byte while it lived (gen_recprog.py,
//! which went with the test oracle in wave 6b-end, finding 531); since then they are omabox's own,
//! edited here.

pub const RUNNING_SH: &str =
    r###"for p; do [ "$(cat "/proc/$p/comm" 2>/dev/null)" = wf-recorder ] || exit 1; done"###;
pub const START_SH: &str = r###"
    first() {
      local d=$1 l us re="zwlr_screencopy_frame_v1[@#][0-9]+[.]ready[(]([0-9]+), ([0-9]+), ([0-9]+)[)]"
      while IFS= read -r l; do
        if [[ $l =~ $re ]]; then
          us=$(( ((BASH_REMATCH[1] << 32) + BASH_REMATCH[2]) * 1000000 + BASH_REMATCH[3] / 1000 ))
          { echo -n "$us "
            perl -MTime::HiRes=clock_gettime,CLOCK_MONOTONIC,CLOCK_REALTIME -e "my @t = map { int(1e6 * clock_gettime(\$_)) } (CLOCK_REALTIME, CLOCK_MONOTONIC, CLOCK_REALTIME); print qq(@t\n)"
          } > "$d/first-frame.part" && mv -f "$d/first-frame.part" "$d/first-frame"
          echo "$us" > "$d/last-frame"
          break
        fi
        [[ $l == "["* ]] || printf "%s\n" "$l"
      done
      exec awk -v f="$d/last-frame" "
        /^\\[/ { if (match(\$0, /zwlr_screencopy_frame_v1[@#][0-9]+[.]ready[(][0-9]+, [0-9]+, [0-9]+[)]/)) {
            s = substr(\$0, RSTART, RLENGTH); sub(/.*ready[(]/, \"\", s); split(s, a, /[,)] ?/)
            printf \"%.0f\\n\", (a[1] * 4294967296 + a[2]) * 1000000 + int(a[3] / 1000) > f; close(f) }
          next }
        { print; fflush() }"
    }
    c=$1; shift
    while [ $# -ge 2 ]; do
      d=$1 o=$2; shift 2
      case $d in */m[0-9]*) rm -rf "$d" ;; esac
      mkdir -p "$d" && rm -f "$d"/raw.mkv "$d"/out.mp4 "$d"/first-frame "$d"/last-frame || exit 125
      (exec >"$d/wf-recorder.log" 2>&1 3>&-; trap - INT QUIT; export WAYLAND_DEBUG=client
        exec setsid -- wf-recorder -y -D -o "$o" -c libx264 -x yuvj420p -p preset=ultrafast -p crf="$c" -f "$d/raw.mkv" 2> >(first "$d")) &
      echo "$!" >&3
    done"###;
pub const ALIVE_SH: &str = r###"for i in 1 2 3 4 5; do sleep 0.1; for p; do kill -0 "$p" 2>/dev/null || exit 1; done; done"###;
pub const TAILS_SH: &str = r###"for d; do timeout 2 tail -n 3 "$d/wf-recorder.log"; done"###;
pub const AUDIO_SH: &str = r###"d=$1
    mkdir -p "$d" && rm -f "$d/audio.mka" "$d/audio.log" || exit 125
    (exec >"$d/audio.log" 2>&1 3>&-; trap - INT QUIT
      exec setsid -- ffmpeg -nostdin -nostats -hide_banner -y -f pulse -name omabox-record -sample_rate 48000 -channels 2 -fragment_size 1920 \
        -i "$2.monitor" -c:a flac "$d/audio.mka") &
    echo "$!" >&3"###;
pub const AUDIO_WAIT_SH: &str = r###"for i in $(seq 50); do grep -q " start: " "$1" 2>/dev/null && exit 0
      [ "$(cat "/proc/$2/comm" 2>/dev/null)" = ffmpeg ] || exit 1; sleep 0.1; done; exit 1"###;
pub const AUDIO_END_SH: &str = r###"p=$1; is() { case $(cat "/proc/$p/stat" 2>/dev/null) in "$p (ffmpeg) "[!Z]*) return 0 ;; esac; return 1; }
    is || exit 0; kill -INT "$p"
    for i in $(seq 50); do is || exit 0; sleep 0.1; done; kill -KILL "$p""###;
pub const AUDIO_START_SED: &str = r###"s/.* start: \([0-9]\{1,12\}\)\.\([0-9]\{6\}\),.*/\1\2/p"###;
pub const FIRSTS_SH: &str =
    r###"for d; do l=$(head -c 200 "$d/first-frame" 2>/dev/null | head -n 1); echo "$l"; done"###;
pub const HOLD_SH: &str = r###"dl=$1 a=("${@:2}")
    while :; do
      ok=1
      for ((i = 0; i < ${#a[@]}; i += 2)); do
        v=""; read -r -n 20 v < "${a[i]}/last-frame" 2>/dev/null || v=""
        [[ $v =~ ^[0-9]{1,17}$ ]] && [ "$v" -ge "${a[i + 1]}" ] || { ok=0; break; }
      done
      [ $ok = 0 ] || exit 0
      [ "${EPOCHREALTIME//[!0-9]/}" -lt "$dl" ] || exit 3
      sleep 0.01
    done"###;
pub const STILL_SH: &str = r###"for p; do [ "$(cat "/proc/$p/comm" 2>/dev/null)" != wf-recorder ] || { [ -z "$S" ] || kill "-$S" "$p"; R=0; }; done; exit ${R:-1}"###;
pub const WRITE_SH: &str = r###"mkdir -p "${1%/*}" && cat > "$1""###;
pub const SHEETDIR_SH: &str = r###"rm -rf "$1" && mkdir -p "$1""###;
pub const BYTES_H_AWK: &str = r###"BEGIN { if (b < 1048576) printf "%d bytes", b; else if (b < 1073741824) printf "%.1f MiB", b / 1048576; else printf "%.1f GiB", b / 1073741824 }"###;
pub const LAYOUT_JQ: &str = r###"
    [.[] | select((.disabled | not) and .width > 0 and .height > 0) | . + {lw: lw, lh: lh}] as $m
    | if ($m | length) == 0 then empty else
      ($m | map(.x) | min) as $x0 | ($m | map(.y) | min) as $y0
      | (($m | map(.x + .lw) | max) - $x0) as $w | (($m | map(.y + .lh) | max) - $y0) as $h
      | ($m | map(.scale) | max) as $s
      | ([1, (35651584 / ($w * $s * $h * $s) | sqrt), (16384 / ([$w, $h] | max) / $s)] | min) as $f
      | ($s * $f) as $k
      | "\($w * $k + 1e-6 | floor) \($h * $k + 1e-6 | floor) \($s) \($f * 1000 | floor / 1000)",
        ($m[] | "\(.name) \((.x - $x0) * $k | round) \((.y - $y0) * $k | round) \([.lw * $k | round, 2] | max) \([.lh * $k | round, 2] | max)")
      end"###;
pub const GEO_MIN_JQ: &str = r###"[.[] | select((.disabled | not) and .width > 0 and .height > 0)] | "\(map(.x) | min) \(map(.y) | min)""###;
pub const GEO_K_AWK: &str = r###"{ print $3 * $4 }"###;
pub const GEO_MAIN_AWK: &str = r###"$1 == m { print $2, $3, $4, $5; exit }"###;
pub const GEO_ONE_JQ: &str = r###".[] | select(.name == $n) | "\(.x) \(.y) \(.scale)""###;
pub const STATUS_JQ: &str = r###"if $r or $s then {recording: $r, stopped: $s, monitors: ($out | split(",")), fps: ($fps | tonumber? // null),
      file: (if $f == "" then null else $f end), audio: $a, started: ($t | tonumber? / 1000000 // null)}
     else {recording: false, stopped: false, monitors: [], fps: null, file: null, audio: false, started: null} end"###;
pub const MARKS_AWK: &str = r###"
      BEGIN { nc = split(cuts, cl, "\n"); for (i = 1; i <= nc; i++) { split(cl[i], c, " "); c1[i] = c[1] * 1000 / fps; c2[i] = c[2] * 1000 / fps } }
      $1 ~ /^[0-9]+$/ { m = ($1 - t0) / 1000; sh = 0
        for (i = 1; i <= nc; i++) if (cl[i] != "") { if (m >= c2[i]) sh += c2[i] - c1[i]; else if (m > c1[i]) sh += m - c1[i] }
        ms = int(m - sh + 0.5); if (ms < 0) ms = 0
        t = $1; sub(/^[^\t]*\t/, ""); printf "%d\t%d.%03d\t%s\n", ms, int(t / 1000000), int(t / 1000) % 1000, $0 }"###;
pub const VTT_AWK: &str = r###"
    function ts(m) { return sprintf("%02d:%02d:%02d.%03d", int(m / 3600000), int(m / 60000) % 60, int(m / 1000) % 60, m % 1000) }
    NR > 1 { n++; ms[n] = $1; sub(/^[^\t]*\t[^\t]*\t/, ""); gsub(/&/, "\\&amp;"); gsub(/</, "\\&lt;"); gsub(/>/, "\\&gt;"); tx[n] = $0 }
    END { print "WEBVTT"
      for (i = 1; i <= n; i++) {
        e = i < n ? ms[i + 1] : dur; if (e <= ms[i]) e = ms[i] + 1000
        printf "\n%d\n%s --> %s\n%s\n", i, ts(ms[i]), ts(e), tx[i] } }"###;
pub const CUT_INPUT_AWK: &str = r###"$1 ~ /^[0-9]{1,17}$/ && $2 ~ /^[0-9]{1,17}$/ { printf "A %.3f %.3f\n", ($1 - f) / 1000, ($2 - f) / 1000 + x }"###;
pub const CUT_MARKS_AWK: &str =
    r###"$1 ~ /^[0-9]{1,17}$/ { printf "A %.3f %.3f\n", ($1 - f) / 1000, ($1 - f) / 1000 }"###;
pub const CUT_AWK: &str = r###"
      function ins(arr, n, s, e,   i) { i = n; while (i > 0 && arr[i, 0] > s) { arr[i + 1, 0] = arr[i, 0]; arr[i + 1, 1] = arr[i, 1]; i-- }; arr[i + 1, 0] = s; arr[i + 1, 1] = e }
      $1 == "A" { na++; ins(a, na - 1, $2 + 0, $3 + 0) }
      $1 == "Fstart" { nf++; fs[nf] = $2 * 1000; fe[nf] = D }
      $1 == "Fend" && nf { fe[nf] = $2 * 1000 }
      $1 == "Sstart" { ns++; ss[ns] = $2 * 1000; se[ns] = -1 }
      $1 == "Send" && ns { se[ns] = $2 * 1000 }
      BEGIN { D = nb * 1000 / fps }
      END {
        for (i = 1; i <= nf; i++) if (fe[i] == "" || fe[i] == 0) fe[i] = D
        # Sound: what is not silence is something happening.
        p = 0
        for (i = 1; i <= ns; i++) { if (ss[i] > p) { na++; ins(a, na - 1, p, ss[i]) }; p = se[i] < 0 ? D : se[i] }
        if (ns && p < D) { na++; ins(a, na - 1, p, D) }
        for (i = 1; i <= nf; i++) {
          c = fs[i]
          for (j = 1; j <= na + 1; j++) {
            if (j <= na) { s = a[j, 0]; e = a[j, 1]; if (e < fs[i] || s > fe[i]) continue } else { s = fe[i]; e = fe[i] }
            if (s > c) {
              b = s < fe[i] ? s : fe[i]
              if (b - c > cut) {
                c1 = int((c + cut) * fps / 1000 + 0.999); c2 = int(b * fps / 1000 + 0.001)
                if (c2 > nb) c2 = nb
                if (c2 > c1) printf "%d %d\n", c1, c2 } }
            if (e > c) c = e
            if (c >= fe[i]) break } } }"###;
pub const OVERLAY_AWK: &str = r###"
    function ts(m) { m = int((m < 0 ? 0 : m) / 10 + 0.5); return sprintf("%d:%02d:%02d.%02d", int(m / 360000), int(m / 6000) % 60, int(m / 100) % 60, m % 100) }
    function vt(us) { return (us - first) / 1000 }
    function px(v) { return sprintf("%.1f", v) }
    function q(v) { return int(v * 4 + (v < 0 ? -0.5 : 0.5)) }
    # A circle of radius r around 0,0 (\p3: quarter pixels), clockwise, or the other way for a hole.
    function circ(r, rev,   c) { c = r * 0.5523
      if (!rev) return sprintf("m %d 0 b %d %d %d %d 0 %d b %d %d %d %d %d 0 b %d %d %d %d 0 %d b %d %d %d %d %d 0 ",
        q(r), q(r), q(c), q(c), q(r), q(r), q(-c), q(r), q(-r), q(c), q(-r), q(-r), q(-c), q(-c), q(-r), q(-r), q(c), q(-r), q(r), q(-c), q(r))
      return sprintf("m %d 0 b %d %d %d %d 0 %d b %d %d %d %d %d 0 b %d %d %d %d 0 %d b %d %d %d %d %d 0 ",
        q(r), q(r), q(-c), q(c), q(-r), q(-r), q(-c), q(-r), q(-r), q(-c), q(-r), q(-r), q(c), q(-c), q(r), q(r), q(c), q(r), q(r), q(c), q(r)) }
    function annulus(r, hw) { return r - hw > 0 ? circ(r + hw, 0) circ(r - hw, 1) : circ(r + hw, 0) }
    function ev(layer, a, b, style, text) { if (b > dur) b = dur; if (a < 0) a = 0
      if (b - a >= 1) printf "Dialogue: %d,%s,%s,%s,,0,0,0,,%s\n", layer, ts(a), ts(b), style, text }
    # The pointer ring at a, still, or moving from (xa, ya) to (xb, yb), with a fade-out at its end.
    function ring_ev(a, b, xa, ya, xb, yb, fade,   p, f) {
      p = (xa == xb && ya == yb) ? "\\pos(" px(xa) "," px(ya) ")" : "\\move(" px(xa) "," px(ya) "," px(xb) "," px(yb) ")"
      f = fade ? "\\fad(0," int(fade) ")" : ""
      ev(1, a, b, "D", "{\\an7" p f "\\1c&H000000&\\1a&H80&\\p3}" annulus(12 * k, 3.5 * k))
      ev(2, a, b, "D", "{\\an7" p f "\\1c&HC83CFF&\\p3}" annulus(12 * k, 2 * k))
      ev(2, a, b, "D", "{\\an7" p f "\\1c&HC83CFF&\\p3}" circ(2 * k, 0)) }
    # ASS reads { } as override blocks and \n \N \h as breaks: braces shown as their wide forms, a
    # zero-width space after each backslash.
    function esc(s) { gsub(/\\/, "\\\342\200\213", s); gsub(/\{/, "\357\275\233", s); gsub(/\}/, "\357\275\235", s); return s }
    function plate(a, b, x, y, an, text, fa, fb, fs,   f) {
      f = (fa != "") ? sprintf("\\fade(%d,%d,255,0,0,%d,%d)", fa, fa, fb < 0 ? 0 : fb, fs) : ""
      ev(3, a, b, "E", "{\\an" an "\\pos(" px(x) "," px(y) ")" f "}" esc(text))
      ev(4, a, b, "K", "{\\an" an "\\pos(" px(x) "," px(y) ")" f "}" esc(text)) }
    # One caption piece at t (peek: a key or a secret stands apart; text runs on).
    function piece(t, s, apart) { pn++; pt[pn] = t; ps[pn] = s; pa[pn] = apart }
    # The pills on show from a to b (each until it is gone, fading 2.5 to 3 s after its last key).
    function pills(a, b,   j, kk, e, al, t3) {
      for (j = np; j >= 1; j--) {
        kk = np - j; e = PT[j] + 3000; if (e > b) e = b; if (e <= a) continue
        t3 = PT[j] + 2500 - a; al = 0
        if (t3 < 0) { al = int(255 * (-t3) / 500); if (al > 255) al = 255; t3 = 0 }
        plate(a, e, AX + AW / 2, AY + AH - 20 * k - kk * (FZ * 1.3 + 2 * PAD + 6 * k), 2, substr(P[j], length(P[j]) > 40 ? length(P[j]) - 39 : 1), al, t3, PT[j] + 3000 - a) } }
    BEGIN { split(area, ar, " "); AX = ar[1]; AY = ar[2]; AW = ar[3]; AH = ar[4]; FZ = int(20 * k + 0.5); PAD = int(6 * k + 0.5); EDGE = int(2 * k + 0.5); if (EDGE < 1) EDGE = 1
      print "[Script Info]"; print "ScriptType: v4.00+"; printf "PlayResX: %d\nPlayResY: %d\n", W, H
      print "ScaledBorderAndShadow: yes"; print "WrapStyle: 2"; print ""
      print "[V4+ Styles]"
      print "Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding"
      printf "Style: D,monospace,%d,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,0,0,7,0,0,0,1\n", FZ
      printf "Style: K,monospace,%d,&H00FFFFFF,&H00FFFFFF,&H191E1616,&H00000000,0,0,0,0,100,100,0,0,3,%d,0,2,0,0,0,1\n", FZ, PAD
      printf "Style: E,monospace,%d,&HFFFFFFFF,&HFFFFFFFF,&H00C83CFF,&H00000000,0,0,0,0,100,100,0,0,3,%d,0,2,0,0,0,1\n", FZ, PAD + EDGE
      print ""; print "[Events]"; print "Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text"
      nk = 0; nr = 0; pn = 0; nm = 0 }
    # A mark (--captions): "M WALL_US<TAB>TEXT".
    $1 == "M" { split($0, f, "\t"); sub(/^M /, "", f[1]); if (f[1] !~ /^[0-9]{1,17}$/) next
      nm++; mt[nm] = vt(f[1]); mx[nm] = substr($0, length(f[1]) + 4); next }
    $1 !~ /^[0-9]{1,17}$/ || $2 !~ /^[0-9]{1,17}$/ || $2 + 0 < $1 + 0 { next }
    $3 == "ptr" {
      if ($4 !~ /^[0-9]{1,5}x[0-9]{1,5}$/) next
      t0 = vt($1); t1 = vt($2); n = 0; wt = 0
      for (i = 5; i <= NF; i++) {
        if ($i == "move") { if ($(i + 1) !~ /^[0-9]{1,5}$/ || $(i + 2) !~ /^[0-9]{1,5}$/) next; n++; op[n] = "move"; a1[n] = $(i + 1); a2[n] = $(i + 2); w[n] = 40; i += 2 }
        else if ($i == "click" || $i == "down" || $i == "up") { n++; op[n] = $i; a1[n] = "left"; w[n] = ($i == "click") ? 30 : 0
          if ($(i + 1) ~ /^(left|right|middle|side|back|extra|forward)$/) a1[n] = $(++i) }
        else if ($i == "sleep") { if ($(i + 1) !~ /^[0-9]{1,6}$/) next; n++; op[n] = "sleep"; w[n] = $(++i) + 0 }
        else if ($i == "scroll" || $i == "hscroll" || $i == "source") { i++; continue }
        else if ($i == "pause") continue
        else next
        wt += w[n] }
      # The tool spends its time at the end of the line (before its first move it starts and
      # connects): each step as long as it takes there, ending ~10 ms before T1; all of it squeezed
      # in when the line is shorter. A motion shows in the next frame drawn (+8 ms); the ring
      # travels to it over the 40 ms before (from the last point, when that was later).
      sc = (wt > t1 - t0 && wt > 0) ? (t1 - t0) / wt : 1; c = 0
      base = t1 - wt * sc; if (base - t0 > 10) base -= 10
      for (j = 1; j <= n; j++) {
        s = base + c * sc + 8; c += w[j]
        if (op[j] == "move") {
          x = (a1[j] + 0.5 - x0) * k; y = (a2[j] + 0.5 - y0) * k
          if (!nk) { cx = x; cy = y }
          st = s - 40; if (nk && st < kt[nk]) st = kt[nk]; if (nk && s < kt[nk]) s = kt[nk]
          nk++; kt[nk] = st; kx[nk] = cx; ky[nk] = cy; ke[nk] = t1 + 3000
          nk++; kt[nk] = s; kx[nk] = x; ky[nk] = y; ke[nk] = t1 + 3000; cx = x; cy = y }
        else if ((op[j] == "click" || op[j] == "down") && nk) { nr++; rt[nr] = s; rx[nr] = cx; ry[nr] = cy; rb[nr] = a1[j] } }
      if (nk) ke[nk] = t1 + 3000
      next }
    # Keys: the lines of one keys call (one T0 T1) share its time, a piece each (text, a character each).
    $3 == "combo" || $3 == "text" || $3 == "secret" {
      g = $1 " " $2; if (g != lastg) flush(); lastg = g; gt0 = vt($1); gt1 = vt($2)
      rest = $0; sub(/^[^ ]+ [^ ]+ [^ ]+ /, "", rest)
      if ($3 == "secret") { if (rest !~ /^[0-9]{1,6}$/) next; rest = substr("****************", 1, rest + 0 > 16 ? 16 : rest + 0) }
      gn++; gk[gn] = $3; gs[gn] = rest; gw += ($3 == "text") ? length(rest) : 1
      next }
    function flush(   i, j, c, s) {
      if (!gn) return
      c = 0
      for (i = 1; i <= gn; i++) {
        if (gk[i] == "text") for (j = 1; j <= length(gs[i]); j++) { c++; piece(gt0 + (gt1 - gt0) * c / gw, substr(gs[i], j, 1), 0) }
        else { c++; piece(gt0 + (gt1 - gt0) * c / gw, gs[i], 1) } }
      gn = 0; gw = 0 }
    END { flush()
      # The pointer: from one point in time to the next, moving or still; still for longer than its
      # 3 s, it fades and goes until the next.
      for (i = 1; i < nk; i++) {
        a = kt[i]; b = kt[i + 1]; if (b <= a) continue
        if (kx[i] != kx[i + 1] || ky[i] != ky[i + 1]) { ring_ev(a, b, kx[i], ky[i], kx[i + 1], ky[i + 1], 0); continue }
        e = ke[i] < b ? ke[i] : b
        if (ke[i] < b) { if (e - 500 > a) ring_ev(a, e - 500, kx[i], ky[i], kx[i], ky[i], 0); s = e - 500 > a ? e - 500 : a; ring_ev(s, e, kx[i], ky[i], kx[i], ky[i], e - s) }
        else ring_ev(a, b, kx[i], ky[i], kx[i], ky[i], 0) }
      if (nk) { a = kt[nk]; e = ke[nk]; if (e - 500 > a) ring_ev(a, e - 500, kx[nk], ky[nk], kx[nk], ky[nk], 0); s = e - 500 > a ? e - 500 : a; ring_ev(s, e, kx[nk], ky[nk], kx[nk], ky[nk], e - s) }
      # Ripples: 12 to 38 px out in 600 ms, fading; the button beside it for 3 s.
      for (i = 1; i <= nr; i++) {
        ev(0, rt[i], rt[i] + 600, "D", "{\\an7\\pos(" px(rx[i]) "," px(ry[i]) ")\\1c&HC83CFF&\\t(\\fscx317\\fscy317\\alpha&HFF&)\\p3}" annulus(12 * k, 1.5 * k))
        tag = rb[i] == "side" ? "back" : rb[i] == "extra" ? "forward" : rb[i]
        if (tag != "left") { ev(3, rt[i], rt[i] + 3000, "E", "{\\an7\\pos(" px(rx[i] + 16 * k) "," px(ry[i] + 14 * k) ")\\fad(0,500)}" tag)
          ev(4, rt[i], rt[i] + 3000, "K", "{\\an7\\pos(" px(rx[i] + 16 * k) "," px(ry[i] + 14 * k) ")\\fad(0,500)}" tag) } }
      # Key captions, as peek joins them.
      np = 0; prev = -1
      for (i = 1; i <= pn; i++) {
        t = pt[i]
        if (prev >= 0 && t > prev) pills(prev, t)
        while (np && PT[1] + 3000 <= t) { for (j = 1; j < np; j++) { P[j] = P[j + 1]; PA[j] = PA[j + 1]; PT[j] = PT[j + 1] }; np-- }
        if (!np || t - PT[np] >= 1500) {
          if (np == 3) { for (j = 1; j < np; j++) { P[j] = P[j + 1]; PA[j] = PA[j + 1]; PT[j] = PT[j + 1] }; np-- }
          np++; P[np] = "" }
        else if (pa[i] || PA[np]) P[np] = P[np] " "
        P[np] = P[np] ps[i]; PA[np] = pa[i]; PT[np] = t; prev = t }
      if (prev >= 0) pills(prev, prev + 3000)
      # Marks at the top, each until the next.
      for (i = 1; i <= nm; i++) plate(mt[i], i < nm ? mt[i + 1] : dur, AX + AW / 2, AY + 20 * k, 8, mx[i], "", 0, 0) }"###;
pub const SHEET_JQ: &str = r###".streams[0] | "\(.nb_read_packets) \(.r_frame_rate | sub("/"; " ")) \(.width) \(.height)""###;
