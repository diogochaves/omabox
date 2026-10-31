//! The jq programs bash's omabox kept in variables, byte for byte as it had them until the port
//! ended (#324): the desktop's windows, layers and changes are jq's answers, so jq runs them.

macro_rules! layer_span_jq { () => { r#"
  def spans($m; $mon): (first($m[]? | select(.name == $mon)) // null) as $o
    | if $o == null or ($o.width // null) == null then false else
        (if ($o.transform // 0) % 2 == 1 then [$o.height, $o.width] else [$o.width, $o.height] end
         | map(. / ($o.scale // 1))) as [$mw, $mh]
        | ($o.reserved // null) as $r
        | if .x <= $o.x and .x + .w >= $o.x + $mw - 1 and .y <= $o.y and .y + .h >= $o.y + $mh - 1 then true
          elif $r == null then false
          else
          (.x <= $o.x + $r[0] and .x + .w >= $o.x + $mw - $r[2] - 1) as $fw
          | (.y <= $o.y + $r[1] and .y + .h >= $o.y + $mh - $r[3] - 1) as $fh
          | if $fh then (if .x <= $o.x then $r[0] == 0 elif .x + .w >= $o.x + $mw - 1 then $r[2] == 0 else true end)
            elif $fw then (if .y <= $o.y then $r[1] == 0 elif .y + .h >= $o.y + $mh - 1 then $r[3] == 0 else true end)
            else false end end end;
  def bar_strip($m; $mon): (first($m[]? | select(.name == $mon)) // null) as $o
    | if (.namespace // "") != "omarchy-bar" or $o == null or ($o.width // null) == null or ($o.reserved // null) == null then .
      else
        (if ($o.transform // 0) % 2 == 1 then [$o.height, $o.width] else [$o.width, $o.height] end
         | map(. / ($o.scale // 1))) as [$mw, $mh]
        | $o.reserved as $r
        | if .x > $o.x or .y > $o.y or .x + .w < $o.x + $mw - 1 or .y + .h < $o.y + $mh - 1 then .
          elif $r[1] > 0 then .h = $r[1]
          elif $r[3] > 0 then .y = .y + .h - $r[3] | .h = $r[3]
          elif $r[0] > 0 then .w = $r[0]
          elif $r[2] > 0 then .x = .x + .w - $r[2] | .w = $r[2]
          else . end end;"# }; }
macro_rules! win_sel_jq { () => { r#"
  def m($s; $loose):
    if $s == "active" then .active
    elif ($s | test("^(address:)?0x[0-9a-fA-F]+$")) then .address == ($s | sub("^address:"; "") | ascii_downcase)
    elif ($s | test("^pid:[0-9]+$")) then .pid == ($s[4:] | tonumber)
    elif ($s | test("^(class|title|initialClass|initialTitle):")) then
      ($s | capture("^(?<k>[A-Za-z]+):(?<v>.*)$")) as $kv | (.[$kv.k] // "") | test($kv.v; "i")
    else (.class | ascii_downcase) as $c | ($s | ascii_downcase) as $w
      | $c == $w or ($c | split(".") | last) == $w or (.title | ascii_downcase | contains($w))
        or ($loose and any($c | split("-")[]; . == $w)) end;
  def hits($loose): [.[] | select(. as $w | all($ARGS.positional[]; . as $s | $w | m($s; $loose)))];
  hits(false) as $h | if $h | length > 0 then $h else hits(true) end"# }; }
macro_rules! win_desc_jq { () => { r#"def n: "\(.address) \(.class) \(.title | tojson)";
  def d: "\(.address) \(.class) \(.title | tojson) at \(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])" + (if .active then ", focused" else "" end);"# }; }
macro_rules! desk_facts_jq { () => { r#"
  def cut: if length > 40 then .[:39] + "…" else . end;
  def win: {address, class, title, workspace: (.workspace.name // null)};
  def wins: [.clients[]? | select(.mapped)];
  def lays: [.layers | objects | .[] | .levels? // {} | .[] | .[]
    | select(.w > 1 and .h > 1) | {ns: (if (.namespace // "") == "" then .address else .namespace end), address, at: {x, y, w, h}}];
  def shown: [.monitors[]? | .activeWorkspace.id, .specialWorkspace.id] | map(select(. != 0 and . != null));
  ($b | wins) as $bw | ($a | wins) as $aw | ($bw + $aw) as $all
  | def name: if (.class // "") == "" then "a window \"\(.title // "" | cut)\"" else "\(.class) \"\(.title | cut)\"" end
      + (. as $w | if any($all[]; .address != $w.address and .class == $w.class and .title == $w.title) then " (\(.address))" else "" end);
    def cname: (if (.class // "") == "" then "a window" else .class end)
      + (. as $w | if any($all[]; .address != $w.address and .class == $w.class) then " (\(.address))" else "" end);
  ($a.active.address // null) as $fa | ($b.active.address // null) as $fb
  | ($a | shown) as $ashown
  | [$aw[] | . as $w | select(all($bw[]; .address != $w.address))] as $opened
  | [ ($opened[] | (.address == $fa) as $f
        | {kind: "window_opened", window: win, focused: $f, rank: 1,
           text: ("window opened" + (. as $w | if any($ashown[]; . == $w.workspace.id) then "" else " on workspace \($w.workspace.name)" end)
             + (if $f then ", focused" else "" end) + ": \(name)")}),
      ($bw[] | . as $w | select(all($aw[]; .address != $w.address))
        | {kind: "window_closed", window: win, rank: 2, text: "window closed: \(name)"}),
      (($b | lays) as $bl | ($a | lays) as $al
        | ([$bl[].ns, $al[].ns] | unique[]) as $n
        | ([$bl[] | select(.ns == $n)] | length) as $nb | [$al[] | select(.ns == $n)] as $an
        | if ($an | length) > $nb then
            {kind: "layer_opened", namespace: $n,
             at: (first($an[] | . as $l | select(all($bl[]; .address != $l.address))) // $an[-1]).at,
             rank: 3, text: "layer \($n) opened"}
          elif ($an | length) < $nb then {kind: "layer_closed", namespace: $n, rank: 4, text: "layer \($n) closed"}
          else empty end),
      (if $fa != $fb then
         {kind: "focus", from: (if $fb then $b.active | win else null end), to: (if $fa then $a.active | win else null end),
          rank: (if $fa != null and any($opened[]; .address == $fa) then null else 5 end),
          text: ("focus: " + (if $fa then $a.active | name else "none" end))}
       else empty end),
      ($aw[] | . as $w | first($bw[] | select(.address == $w.address)) as $o
        | (if $o.title != $w.title then
             {kind: "title", window: ($w | win), from: $o.title, to: $w.title,
              rank: (if $w.address == $fa then 6 else 10 end), text: "\($w | cname) title now \($w.title | cut | tojson)"}
           else empty end),
          (if ($o.workspace.name // null) != ($w.workspace.name // null) then
             {kind: "window_moved", window: ($w | win), from: ($o.workspace.name // null), to: ($w.workspace.name // null),
              rank: 8, text: "\($w | name) moved to workspace \($w.workspace.name)"}
           else empty end),
          (if $o.floating != $w.floating or (($o.fullscreen // 0) > 0) != (($w.fullscreen // 0) > 0) then
             {kind: "window_state", window: ($w | win), floating: $w.floating, fullscreen: (($w.fullscreen // 0) > 0), rank: 9,
              text: ("\($w | name) " + ([
                (if (($o.fullscreen // 0) > 0) != (($w.fullscreen // 0) > 0) then (if ($w.fullscreen // 0) > 0 then "now fullscreen" else "out of fullscreen" end) else empty end),
                (if $o.floating != $w.floating then (if $w.floating then "now floating" else "now tiled" end) else empty end)] | join(", ")))}
           else empty end)),
      (($b.workspace.name // null) as $wb | ($a.workspace.name // null) as $wa
        | if $wb != $wa then {kind: "workspace", from: $wb, to: $wa, rank: 7, text: "workspace now \($wa)"} else empty end)
    ] as $all_changes
  | ($all_changes | map(select(.rank != null)) | sort_by(.rank) | map(.text)) as $texts
  | (reduce $texts[] as $t ({shown: [], open: true};
       if .open and (.shown | length) < 3 and ((.shown | length) == 0 or ((.shown | join("; ") | length) + 2 + ($t | length)) <= 160)
       then .shown += [$t] else .open = false end)) as $cap
  | (($texts | length) - ($cap.shown | length)) as $more
  | {changes: ($all_changes[:50] | map(del(.rank, .text))),
     line: ($cap.shown | join("; ") + (if $more > 0 then "; +\($more) more (omabox events --since \($since // "0"))" else "" end))}"# }; }
macro_rules! layer_list_jq { () => { r#"[to_entries[] | .key as $m | .value.levels // {} | to_entries[] | (.key | tonumber) as $lv
  | .value[] | {namespace, address, pid, x, y, w, h, monitor: $m,
    level: (["background", "bottom", "top", "overlay"][$lv] // "level \($lv)")}]"# }; }
macro_rules! jq_mon {
    () => {
        r#"def lw: if (.transform // 0) % 2 == 1 then .height / .scale else .width / .scale end;
def lh: if (.transform // 0) % 2 == 1 then .width / .scale else .height / .scale end; "#
    };
}

/// Whether a layer may take input on part of it only (#263, finding 398; #311, finding 421); and
/// bar_strip: Omarchy 4.5.0-rc2's bar surface spans its screen, input on the strip along the edge its
/// output reserves only (#407), so it is that strip.
pub const LAYER_SPAN_JQ: &str = layer_span_jq!();

/// win_annotate's: the mapped windows, their place, cover and layers (finding 81, #263).
pub const WIN_JQ: &str = concat!(
    layer_span_jq!(),
    r#"
  def overlaps($o): .at[0] < $o.at[0] + $o.size[0] and $o.at[0] < .at[0] + .size[0]
    and .at[1] < $o.at[1] + $o.size[1] and $o.at[1] < .at[1] + .size[1];
  def above($o; $t):
    if $o.workspace.id < 0 and $t.workspace.id > 0 then true
    elif $o.workspace.id != $t.workspace.id then false
    elif ($o.fullscreen > 0) != ($t.fullscreen > 0) then $o.fullscreen > 0
    elif $o.floating != $t.floating then $o.floating
    else $o.z > $t.z end;
  [$l | objects | to_entries[] | .key as $mon | .value | .levels? // {}
   | (.["2"] // [] | map(. + {layer: "top"})), (.["3"] // [] | map(. + {layer: "overlay"}))
   | .[] | bar_strip($m; $mon) | select(.w > 1 and .h > 1) | {namespace, pid, layer, at: [.x, .y], size: [.w, .h], monitor: $mon, certain: (spans($m; $mon) | not)}] as $L
  | ([$m[] | .activeWorkspace.id, .specialWorkspace.id] | map(select(. != 0))) as $shown
  | [$c | map(select(.mapped)) | to_entries[] | .value + {z: .key}
     | .onscreen = (.workspace.id as $w | any($shown[]; . == $w) and (.hidden | not))] as $all
  | [$all[] | . as $t | {address, stableId, class, title, initialClass, initialTitle, pid,
      workspace: .workspace.name, at, size, floating, fullscreen: (.fullscreen > 0), xwayland,
      active: (.address == $a.address), onscreen,
      cover: [$all[] | select(.address != $t.address and .onscreen and above(.; $t) and overlaps($t))
              | {address, class, title, at, size}]}
     + if $l == null then {} else {layers: (if $t.onscreen | not then [] else
         any($all[]; .onscreen and .workspace.id == $t.workspace.id and .fullscreen >= 2) as $fs
         | [$L[] | select((.layer == "overlay" or ($fs | not)) and overlaps($t))] end)} end]"#
);

/// A window selector's matches (#20, #244).
pub const WIN_SEL_JQ: &str = win_sel_jq!();

/// wait window's words for a window: n what it is, d where it is too.
pub const WIN_DESC_JQ: &str = win_desc_jq!();

/// What changed on the desktop between two snapshots (#274).
pub const DESK_FACTS_JQ: &str = desk_facts_jq!();

/// What got the input, from a snapshot (findings 398, 421); `(WIN)` stands for WIN_JQ. A window in
/// real fullscreen at the point, on a shown workspace, covers the top level there (#409, finding 561).
pub const DESK_TARGET_JQ: &str = concat!(
    layer_span_jq!(),
    r#"
  def cut: if length > 40 then .[:39] + "…" else . end;
  def win: {address, class, title, workspace: ((.workspace | objects | .name) // .workspace)};
  def name: (if (.class // "") == "" then "a window" else .class end) + " \"\(.title // "" | cut)\"";
  def target($k; $w; $n; $at; $t): {kind: $k, window: $w, namespace: $n, at: $at, certain: true, text: $t};
  $s.clients as $c | $s.monitors as $m | $s.active as $a | $s.layers as $l
  | if $what == "keys" then
      if $kbl != "" then target("layer"; null; ($kbl | split(", ")[0]); null; "the keys went to layer \($kbl) (it has the keyboard)")
      elif ($a.address // null) then target("window"; ($a | win); null; null; "the keys went to \($a | name) (focused)")
      else target("none"; null; null; null; "the keys went to no window (none focused)") end
    elif $what == "run" then
      (first($c[]? | select(.mapped and .pid == $pid)) // null) as $w
      | if $w then target("window"; ($w | win); null; null; "its window is \($w | name)")
        else target("none"; null; null; null; "no window from pid \($pid) yet") end
    else
      (if $what == "drag" then "the drag from \($x),\($y) started" elif $what == "scroll" then "the scroll at \($x),\($y) was"
       else "the click at \($x),\($y) was" end) as $say
      | def at($o): $x >= $o.x and $x < $o.x + $o.w and $y >= $o.y and $y < $o.y + $o.h;
      ([$m[]? | .activeWorkspace.id] | map(select(. != null and . > 0))) as $act
      | any($c[]?; .mapped and (.hidden | not) and (.fullscreen // 0) >= 2 and (.workspace.id as $i | any($act[]; . == $i))
          and at({x: .at[0], y: .at[1], w: .size[0], h: .size[1]})) as $fs
      | [$l | objects | to_entries[] | .key as $mon | .value | .levels? // {} | to_entries[] | (.key | tonumber) as $lv | .value | reverse[]
        | select($lv != 2 or ($fs | not))
        | bar_strip($m; $mon) | select(.w > 1 and .h > 1 and at(.))
        | {lv: $lv, ns: (if (.namespace // "") == "" then .address else .namespace end), span: ($lv >= 2 and spans($m; $mon))}] as $L
      | ([(WIN) | .[] | select(.onscreen and at({x: .at[0], y: .at[1], w: .size[0], h: .size[1]})
            and all(.cover[]; at({x: .at[0], y: .at[1], w: .size[0], h: .size[1]}) | not))] | first // null) as $w
      | ([$L[] | select(.lv >= 2)] | sort_by(-.lv)) as $tops
      | ($tops | map(select(.span | not)) | first // null) as $top
      | ($tops | map(select(.span)) | first // null) as $maybe
      | if $top then target("layer"; null; $top.ns; null; "\($say) on layer \($top.ns)"
            + (if $w then " (over \($w | name))" else "" end))
        else
          (if $w then target("window"; ($w | win); null; {x: ($x - $w.at[0]), y: ($y - $w.at[1])};
              "\($say) on \($w | name) (its \($x - $w.at[0]),\($y - $w.at[1]))")
           elif ($L | map(select(.lv < 2)) | length) > 0 then
             ($L | map(select(.lv < 2)) | sort_by(-.lv) | first) as $b | target("layer"; null; $b.ns; null; "\($say) on no window (layer \($b.ns))")
           else target("none"; null; null; null; "\($say) on no window or layer") end)
          | if $maybe then .certain = false
              | .text += ", or possibly on layer \($maybe.ns) (it spans the screen: if one of its items, a toast, is there)"
            else . end
        end
    end"#
);

/// screen_layer_of: the window at a screen point, from a snapshot, as DESK_TARGET_JQ finds it (the
/// topmost shown there, nothing over it at the point), with WIN_JQ's `layers`; `(WIN)` stands for
/// WIN_JQ (#361).
pub const SCREEN_WIN_JQ: &str = r#"$s.clients as $c | $s.monitors as $m | $s.active as $a | $s.layers as $l
  | def at($o): $x >= $o.x and $x < $o.x + $o.w and $y >= $o.y and $y < $o.y + $o.h;
    def r: {x: .at[0], y: .at[1], w: .size[0], h: .size[1]};
  first((WIN) | .[] | select(.onscreen and at(r) and all(.cover[]; at(r) | not))) // empty"#;

/// The layers of `hyprctl -j layers`, one object each.
pub const LAYER_LIST_JQ: &str = layer_list_jq!();

/// A monitor's logical size, turned with its transform (finding 311).
pub const JQ_MON: &str = jq_mon!();

/// A program by its name, for `_rust-jq NAME`: the suite runs LAYER_SPAN_JQ's spans with a jq of
/// its own (t_unit_window_select).
pub fn by_name(n: &str) -> Option<&'static str> {
    match n {
        "LAYER_SPAN_JQ" => Some(LAYER_SPAN_JQ),
        _ => None,
    }
}

/// Inline programs of bash's omabox's functions, as they were there (#324).
/// win_lines: a window a line.
pub const WIN_LINES_JQ: &str = r#"if length == 0 then "  (none)" else .[] | "  \(.address)  \(.class)  \(.title | tojson)  workspace \(.workspace)" end"#;
/// win_desc: `0x... class "title"`.
pub const WIN_DESC_ONE_JQ: &str = r#""\(.address) \(.class) \(.title | tojson)""#;
/// layer_at: where a layer is drawn, a 1x1 one not (finding 146).
pub const LAYER_AT_JQ: &str = r#"first(.. | objects | select(.namespace? == $n and .w > 1 and .h > 1)
    | "\(.x),\(.y) \(.w)x\(.h)") // empty"#;
/// wait layer's answer ($want present or gone, $mon one monitor or ""): where the drawn layers of
/// namespace $n are, each with its monitor (#316); for --monitor, the others named when it is drawn
/// elsewhere, and a monitor the box does not have said.
pub const LAYER_WAIT_JQ: &str = r#"[keys_unsorted[]] as $ms
  | [to_entries[] | .key as $m | .value.levels // {} | .[] | .[]
     | select(.namespace == $n and .w > 1 and .h > 1) | . + {monitor: $m}] as $all
  | ($all | map(select($mon == "" or .monitor == $mon))) as $on
  | def at: "at \(.x),\(.y) \(.w)x\(.h) on \(.monitor)";
    def where: if length == 1 then .[0] | at else "\(length) layers: \(map(at) | join("; "))" end;
    (if $mon != "" and all($ms[]; . != $mon) then "no monitor \($mon) in the box (\($ms | join(", ")))" else null end) as $why
  | if $want == "gone" then
      if $on == [] then "yes" + (if $why then " \($why)" else "" end) else "no \($on | where)" end
    elif $on != [] then "yes \($on | where)"
    elif $why then "no \($why)"
    elif $all != [] then "no \($all | where)"
    else "no" end"#;
/// win_answer focused (after WIN_DESC_JQ).
pub const WIN_FOCUSED_JQ: &str = r#"
        (first(.[] | select(.active)) // null) as $f
        | if length == 0 then "no"
          elif $f == null then if length == 1 then "no \(.[0] | n) is there" else "no \(length) matching, none focused" end
          else "yes \($f | d)" + (if length > 1 then " (1 of \(length) matching)" else "" end) end"#;
/// win_answer present (after WIN_DESC_JQ).
pub const WIN_PRESENT_JQ: &str = r#"
        if length == 0 then "no" elif length == 1 then "yes \(.[0] | d)"
        else "yes \(length) windows: \(map(d) | join("; "))" end"#;
/// poll_until --json.
pub const POLL_JSON_JQ: &str = r#"{result: $res, condition: ($cond | sub("_probe$"; "") | sub("^win$"; "window")), message: $msg, elapsed: ($t | tonumber / 1000),
        detail: (if $d == "" then null else $d end)}"#;
/// cursor_now: "X Y".
pub const CURSOR_JQ: &str = r#"select((.x | type) == "number" and (.y | type) == "number") | "\(.x | floor) \(.y | floor)""#;
/// settle_end --json.
pub const SETTLE_JSON_JQ: &str = r#"
      def secs: if . == "" or . == "-" then null else tonumber / 1000 end;
      def box: if . == "-" then null else split(",") | map(tonumber) | {x: .[0], y: .[1], w: .[2], h: .[3]} end;
      {result: $res, condition: $cond, message: $msg, elapsed: ($t | secs), first_change: ($first | secs),
       last_change: ($last | secs), change: ($ch | box), late_changes: ($late | box),
       ignored: (($ig | box) as $b | if $b then $b + {why: $why} else null end),
       frames: (if $frames == "" then null else $frames | tonumber end)}
      + if $facts == "" then {} else {changes: $changes, target: $target, events_since: $evs} end
      + if $exp == "" then {} else {expect: ($exp | fromjson)} end"#;
/// desk_snap: the five answers as one object.
pub const SNAP_JQ: &str = r#"select(length == 5 and (.[0] | type) == "array" and (.[1] | type) == "object"
    and (.[2] | type) == "object" and (.[4] | type) == "array")
    | {events: ($s | tonumber? // null), clients: .[0], layers: .[1], active: .[2], workspace: .[3], monitors: .[4]}"#;
/// windows --json.
pub const WINDOWS_JSON_JQ: &str = r#"def r: . + {x: .at[0], y: .at[1], w: .size[0], h: .size[1]};
      map(r | .cover |= map(r) | if .layers then .layers |= map(r) else . end)"#;
/// windows_table.
pub const WINDOWS_TABLE_JQ: &str = r#"def pad($n): tostring | . + (" " * ($n - length) // "");
    .[] | [(.address | pad(16)), (.pid | pad(8)), (.workspace | pad(10)),
      ((if .onscreen | not then "off-screen" elif .cover != [] then "covered" else "on-screen" end)
        + (if .active then ",active" else "" end) | pad(20)),
      ("\(.size[0])x\(.size[1]) at \(.at[0]),\(.at[1])" | pad(22)), (.class | pad(16)), (.title | tojson)] | join(" ")"#;
/// windows_layers.
pub const WINDOWS_LAYERS_JQ: &str = r#"[.[] | .address as $a | .layers[]? | {k: [.namespace, .layer, .at, .size, (.certain != false), .monitor], a: $a}]
    | group_by(.k)[] | .[0].k as [$n, $l, $at, $sz, $c, $mo]
    | "layer \($n | tojson) (\($l) \($sz[0])x\($sz[1]) at \($at[0]),\($at[1])\(if $mo then " on \($mo)" else "" end)) "
      + if $c then "is over \(map(.a) | join(", ")): where it takes input, input there goes to it"
        else "spans the screen, possibly over \(map(.a) | join(", ")): input goes to it only where one of its items (a toast, a dock icon) is, elsewhere to the window" end"#;
/// layer_select: the layers SEL names.
pub const LAYER_HITS_JQ: &str = r#"[.[] | select(
      if ($s | startswith("address:")) then .address == ($s | ltrimstr("address:"))
      elif ($s | startswith("pid:")) then (.pid | tostring) == ($s | ltrimstr("pid:"))
      else .namespace == $s end)]"#;
/// layer_lines.
pub const LAYER_LINES_JQ: &str = r#"if length == 0 then "  (none)" else .[] | "  \(.address)  \(.namespace | tojson)  pid \(.pid)  \(.level) \(.w)x\(.h) at \(.x),\(.y) on \(.monitor)" end"#;
/// layer_desc.
pub const LAYER_DESC_JQ: &str =
    r#""\(.namespace | tojson) (pid \(.pid), \(.w)x\(.h) at \(.x),\(.y))""#;
/// layer_read: its pid and place.
pub const LAYER_FIELDS_JQ: &str = r#""\(.pid) \(.w) \(.h) \(.x) \(.y)""#;
/// layer_read: its pid and size (after LAYER_LIST_JQ).
pub const LAYER_TIED_JQ: &str = r#" | map(select(.pid == $l.pid and .w == $l.w and .h == $l.h))"#;
/// tree: its app's other windows of its size (#302).
pub const TREE_TWINS_JQ: &str =
    r#"[.[] | select(.pid == $w.pid and .address != $w.address and .size == $w.size)] | length"#;
/// extent (after JQ_MON).
pub const EXTENT_JQ: &str = r#"[.[] | select(.disabled | not)] |
    "\(map(.x + lw) | max | floor)x\(map(.y + lh) | max | floor)""#;
/// main_screen, interactive.
pub const MAIN_SCREEN_I_JQ: &str =
    r#"map(select((.disabled | not) and (.name | startswith("WAYLAND-")))) | .[0]"#;
/// main_screen, headless.
pub const MAIN_SCREEN_H_JQ: &str =
    r#"map(select((.disabled | not) and (.name == "HEADLESS-2" or .name == "WAYLAND-1"))) | .[0]"#;
/// box_screen (after main_screen).
pub const BOX_SCREEN_JQ: &str =
    r#" // empty | "\(.name) \(.width)x\(.height)@\(.refreshRate * 100 | round / 100)""#;
/// win_scale.
pub const WIN_SCALE_JQ: &str = r#"first(.[] | select(.address == $a) | .monitor) as $id | first($m[] | select(.id == $id) | .scale) // 1"#;
/// A window by its address.
pub const BY_ADDRESS_JQ: &str = r#"first(.[] | select(.address == $a)) // empty"#;
/// pixel: a point on no monitor (#161; after JQ_MON).
pub const PIXEL_OFF_JQ: &str = r#"[.[] | select(.disabled | not)] as $m |
      [range(0; $p | length; 2) | [$p[.], $p[. + 1]] | select(. as [$x, $y] | $x < $lw and $y < $lh and ($m | all(.[];
        $x < .x or $y < .y or $x >= .x + (lw | floor) or $y >= .y + (lh | floor))))] |
      if length == 0 or ($m | length) == 0 then empty
      else "\(.[0][0]),\(.[0][1]) is not on any monitor (\($m | map("\(.name) \(.x),\(.y) \(lw | floor)x\(lh | floor)") | join(", ")))" end"#;
/// wait --window: its place as X,Y,W,H.
pub const WIN_REGION_JQ: &str = r#""\(.at[0]),\(.at[1]),\(.size[0]),\(.size[1])""#;
/// A window's size, WxH.
pub const WIN_SIZE_JQ: &str = r#""\(.size[0])x\(.size[1])""#;
/// pixel: a window's size, W H.
pub const WIN_SIZE2_JQ: &str = r#""\(.size[0]) \(.size[1])""#;

/// layer_select: the ones drawn (over 1x1, finding 146).
pub const LAYER_BIG_JQ: &str = r#"map(select(.w > 1 and .h > 1))"#;

/// win_blocked: why a point of a window cannot be clicked, or nothing.
pub const WIN_BLOCKED_JQ: &str = r#"(.at[0] + $x) as $px | (.at[1] + $y) as $py
    | if .onscreen | not then "not on screen (workspace \(.workspace))"
      else first(.cover[] | select($px >= .at[0] and $px < .at[0] + .size[0] and $py >= .at[1] and $py < .at[1] + .size[1])
        | "covered there by \(.address) \(.class) \(.title | tojson)") // empty end"#;
/// win_layer: the layer surface over a point of a window (#263, findings 398, 421).
pub const WIN_LAYER_JQ: &str = r#"(.at[0] + $x) as $px | (.at[1] + $y) as $py
    | [.layers[]? | select($px >= .at[0] and $px < .at[0] + .size[0] and $py >= .at[1] and $py < .at[1] + .size[1])]
    | select(length > 0) | (map(select(.certain != false)) | last) // last
    | "\(if .certain == false then "possible" else "certain" end) \(.namespace | tojson) \(.layer) \(.size[0])x\(.size[1]) at \(.at[0]),\(.at[1])""#;
/// win_point: the point inside the window.
pub const WIN_INSIDE_JQ: &str = r#"$x < .size[0] and $y < .size[1]"#;
/// win_point: its screen point.
pub const WIN_AT_JQ: &str = r#""\(.at[0] + $x) \(.at[1] + $y)""#;
/// win_raise: the active window's address.
pub const ACTIVE_ADDR_JQ: &str = r#".address // empty"#;

/// zero_note: the outputs listed 0x0 (#213).
pub const ZERO_NOTE_JQ: &str =
    r#"[.[] | select((.disabled | not) and (.width == 0 or .height == 0)) | .name] | join(", ")"#;
/// shot --json (#273, audit J2): what the shot is.
pub const SHOT_SAID_JQ: &str = r#"
    def n: if . == "" then null else tonumber end;
    ($p != "") as $img
    | {path: (if $img then $p else null end), kind: $k,
       region: (if $img then {x: ($x | n), y: ($y | n), w: ($w | n), h: ($h | n)} else null end),
       image: (if $img then {w: ($iw | n), h: ($ih | n)} else null end),
       scale: (if $img then (($iw | n) / ($w | n) * 10000 | round / 10000) else null end),
       window: ($win | if . == null then null else {address, class, title, workspace, x: .at[0], y: .at[1], w: .size[0], h: .size[1]} end),
       monitor: (if $mon == "" then null else $mon end), fit: ($fit | n), zoom: ($zoom | n),
       changed: (if $ch == 1 then {result: $res, since: (if $since == "" then null else $since end),
         why: (if $why == "" then null else $why end)} else null end)}"#;
/// hyprctl notify (#359): where Hyprland draws its notifications, the focused monitor's top right,
/// as "NAME X Y W H": the right half of its top third (after JQ_MON).
pub const NOTIFY_CORNER_JQ: &str = r#"first(.[] | select(.focused and (.disabled | not))) |
      (lw | floor) as $w | (($w / 2) | floor) as $l |
      "\(.name) \(.x + $l) \(.y) \($w - $l) \(lh / 3 | floor)""#;
/// shot --monitor (#122): that monitor's part of the layout (after JQ_MON).
pub const SHOT_MON_JQ: &str = r#"first(.[] | select(.name == $n and (.disabled | not))) |
      "\(.x) \(.y) \(lw | floor) \(lh | floor)""#;
/// shot --monitor: the monitors there are.
pub const MON_NAMES_JQ: &str = r#"map(.name) | join(", ")"#;
/// shot --monitor: the one named, for zero_note.
pub const MON_NAMED_JQ: &str = r#"map(select(.name == $n))"#;
/// shot --fit (#227): the highest scale of the outputs captured (after JQ_MON).
pub const SHOT_SCALE_JQ: &str = r#"[.[] | select((.disabled | not) and .x < $x + $w and $x < .x + lw and .y < $y + $h and $y < .y + lh) | .scale]
         | max // 1"#;
/// shot --window: whether it is on screen, what covers it.
pub const WIN_STATE_JQ: &str = r#"if .onscreen | not then ", not on screen"
      elif .cover != [] then ", covered by \(.cover[0].address) \(.cover[0].class) \(.cover[0].title | tojson)"
        + (if (.cover | length) > 1 then " and \(.cover | length - 1) more" else "" end)
      else "" end"#;
/// shot --window: where it is, X,Y.
pub const WIN_AT2_JQ: &str = r#""\(.at[0]),\(.at[1])""#;
/// calibrate: a full shot's size (finding 299; after JQ_MON).
pub const CALIB_SIZE_JQ: &str = r#"[.[] | select(.disabled | not)] | (map(.scale) | max) as $s |
         "\(map(.x + lw) | max * $s + 0.5 | floor)x\(map(.y + lh) | max * $s + 0.5 | floor)""#;
/// calibrate --answer --json (#273, audit J8): the verdict.
pub const CALIB_JSON_JQ: &str = r#"
      def num: tonumber? // null;
      ($r | split(" ")) as $v
      | {result: (if $v[0] == "one" or $v[0] == "fit" or $v[0] == "up" then $v[0] else "none" end),
         card: {w: ($w | num), h: ($h | num)}, marks: $n, scale: null, scale_x: null, scale_y: null, fit: null,
         fit_size: null, error_px: null, worst: null, tolerance_px: null, off: {}, shot_fit: ($cur | num)}
      + if $v[0] == "one" then {scale: 1, error_px: ($v[1] | num), worst: $v[2]}
        elif $v[0] == "fit" then {scale: ($v[1] | num), scale_x: ($v[2] | num), scale_y: ($v[3] | num), fit: ($v[4] | num),
          fit_size: $v[5], error_px: ($v[6] | num), worst: $v[7]}
        elif $v[0] == "up" then {scale: ($v[1] | num)}
        elif $v[0] == "off" then {scale: (($v[1] | num) as $s | if $s == 0 then null else $s end), tolerance_px: ($v[2] | num),
          off: ([$v[3:][] | select(test("^[A-Z]:[0-9]+$")) | split(":") | {(.[0]): (.[1] | tonumber)}] | add // {})}
        else {} end"#;

/// calib_marks (#188): one mark in each cell of a 3 x 2 grid, at a random point of its middle (awk's
/// rand: the same seed, the same marks).
pub const CALIB_MARKS_AWK: &str = r#"BEGIN {
    srand(seed)
    for (i = 0; i < n; i++) p[i] = i
    for (i = n - 1; i > 0; i--) { j = int(rand() * (i + 1)); t = p[i]; p[i] = p[j]; p[j] = t }
    for (i = 0; i < n; i++) {
      x = w * (i % 3 + 0.2 + 0.6 * rand()) / 3; y = h * (int(i / 3) + 0.2 + 0.6 * rand()) / 2
      if (x < 0.08 * w) x = 0.08 * w; if (x > 0.92 * w) x = 0.92 * w
      if (y < 0.08 * h) y = 0.08 * h; if (y > 0.92 * h) y = 0.92 * h
      printf "%c %d %d\n", 65 + p[i], x, y
    }}"#;
/// calib_fit: which scale maps the readings onto the marks (awk).
pub const CALIB_FIT_AWK: &str = r#"
  function hyp(a, b) { return sqrt(a * a + b * b) }
  BEGIN {
    long = w > h ? w : h; short = w > h ? h : w; tol = long / 160; if (tol < 8) tol = 8
    split(marks, ml, "\n"); for (i in ml) if (split(ml[i], f, " ") == 3) { mx[f[1]] = f[2]; my[f[1]] = f[3] }
    n = 0; split(reads, rl, "\n")
    for (i = 1; i in rl; i++) if (split(rl[i], f, " ") == 3 && (f[1] in mx)) { n++; L[n] = f[1]; rx[n] = f[2]; ry[n] = f[3] }
    if (n == 0) { print "none"; exit 1 }
    m = 0; worst = L[1]
    for (i = 1; i <= n; i++) { e = hyp(rx[i] - mx[L[i]], ry[i] - my[L[i]]); if (e > m) { m = e; worst = L[i] } }
    if (m <= tol) { printf "one %d %s\n", m + 0.5, worst; exit 0 }
    k = 0
    for (i = 1; i <= n; i++) { r[++k] = rx[i] / mx[L[i]]; r[++k] = ry[i] / my[L[i]] }
    for (i = 2; i <= k; i++) { v = r[i]; for (j = i - 1; j >= 1 && r[j] > v; j--) r[j + 1] = r[j]; r[j + 1] = v }
    s0 = k % 2 ? r[(k + 1) / 2] : (r[k / 2] + r[k / 2 + 1]) / 2
    if (s0 <= 0) { printf "off 0 %d", tol; for (i = 1; i <= n; i++) printf " %s:%d", L[i], hyp(rx[i] - mx[L[i]], ry[i] - my[L[i]]) + 0.5; print ""; exit 1 }
    # (None near it: all of them.)
    for (pass = 1; pass <= 2 && !dx; pass++)
      for (i = 1; i <= n; i++) {
        x = mx[L[i]]; y = my[L[i]]
        if (pass == 1 && hyp(rx[i] / s0 - x, ry[i] / s0 - y) > 3 * tol) continue
        cx += x * rx[i]; dx += x * x; cy += y * ry[i]; dy += y * y
      }
    s = (cx + cy) / (dx + dy); sx = cx / dx; sy = cy / dy
    out = ""
    for (i = 1; i <= n; i++) {
      e = hyp(rx[i] / s - mx[L[i]], ry[i] / s - my[L[i]])
      if (e > tol) out = out sprintf(" %s:%d", L[i], e + 0.5)
    }
    if (out != "" || s < 0.2) { printf "off %.3f %d%s\n", s, tol, out; exit 1 }
    if (s > 1) { printf "up %.3f\n", s; exit 1 }
    N = int(s * long / 8 + 0.5) * 8; k = N / long; S = int(short * k + 0.5)
    m = 0; worst = L[1]
    for (i = 1; i <= n; i++) { e = hyp(rx[i] / k - mx[L[i]], ry[i] / k - my[L[i]]); if (e > m) { m = e; worst = L[i] } }
    printf "fit %.3f %.3f %.3f %d %s %d %s\n", s, sx, sy, N, w >= h ? N "x" S : S "x" N, m + 0.5, worst
  }"#;

// --- wave 4a: monitor, output, mode, gpu, cpu, audio, ports, clip (as bash's omabox had them) ---
/// parse_mode host: the user's focused monitor's mode.
pub const HOST_MODE_JQ: &str = r#"map(select(.focused)) | .[0] // empty |
      "\(.width)x\(.height)@\(.refreshRate * 100 | round / 100)""#;

/// main_scale_live (after main_screen).
pub const MAIN_SCALE_JQ: &str = r#" // empty | .scale * 100 | round / 100"#;

/// monitors_record: box.json's list of the monitors.
pub const MONITORS_RECORD_JQ: &str = r#".monitors = ($m | map(select(.disabled | not)) | map({name, mode: "\(.width)x\(.height)@\(.refreshRate * 100 | round / 100)",
    scale, x, y}))"#;

/// placed_as: the layout's rows, the main screen by its name.
pub const PLACED_ROWS_AWK: &str = r#"$1 == "main" { $1 = m } { print $1, $2, $3 }"#;

/// placed_as: each row where Hyprland has it.
pub const PLACED_AS_JQ: &str = r#"[.[] | "\(.name) \(.x) \(.y)"] as $live
    | all($rows | split("\n")[] | select(. != ""); . as $r | $live | index($r) != null)"#;

/// monitors_place: the ones that moved.
pub const PLACE_MOVED_JQ: &str = r#"map(. as $n | select(any($a[]; .name == $n.name and (.x != $n.x or .y != $n.y)))
    | "\(.name) to \(.x),\(.y)") | join(", ")"#;

/// mode --json (#273, audit J6).
pub const MODE_JSON_JQ: &str = r#"{name: $n, mode: $m, scale: ($s | tonumber? // 1)}"#;

/// mode: box.json's size and scale.
pub const MODE_RECORD_JQ: &str = r#".size = $s | .scale = $sc"#;

/// output drop: the monitors still on.
pub const OUTPUT_LEFT_JQ: &str =
    r#"map(select((.disabled | not) and .name != "FALLBACK") | .name) | join(", ")"#;

/// output_drop: its line.
pub const DROP_REC_JQ: &str = r#"first(.[] | select(.name == $n and (.disabled | not))) |
    "\(.name) \(.width)x\(.height)@\(.refreshRate * 100 | round / 100) \(.x)x\(.y) \(.scale)""#;

/// output_drop: the monitors there are, Hyprland's stand-in left out.
pub const MON_NAMES_NOFB_JQ: &str = r#"map(select(.name != "FALLBACK") | .name) | join(", ")"#;

/// A monitor of that name is listed.
pub const ANY_NAME_JQ: &str = r#"any(.[]; .name == $n)"#;

/// output_back_one: its mode now.
pub const BACK_LINE_JQ: &str = r#"first(.[] | select(.name == $n and (.disabled | not))) |
      "\(.name) \(.width)x\(.height)@\(.refreshRate * 100 | round / 100)""#;

/// monitor list: the dropped ones (#163).
pub const LIST_DROPPED_JQ: &str = r#"[inputs | split(" ") | select(length == 4) | {name: .[0], mode: .[1],
      scale: (.[3] | tonumber? // .), x: (.[2] | split("x")[0] | tonumber? // 0), y: (.[2] | split("x")[1] | tonumber? // 0), dropped: true}]"#;

/// monitor list --json.
pub const LIST_JSON_JQ: &str = r#"map(select(.disabled | not)) | map({name, mode: "\(.width)x\(.height)@\(.refreshRate * 100 | round / 100)", scale, x, y,
        workspace: .activeWorkspace.name}) + $dr"#;

/// monitor list: a line each.
pub const LIST_TABLE_JQ: &str = r#"(map(select(.disabled | not))[] | "\(.name) \(.width)x\(.height)@\(.refreshRate * 100 | round / 100) \(.scale) \(.x),\(.y) \(.activeWorkspace.name)"),
        ($dr[] | "\(.name) \(.mode) \(.scale) \(.x),\(.y) (dropped)")"#;

/// monitor_add: its line as Hyprland made it.
pub const ADD_LINE_JQ: &str = r#"first(.[] | select(.name == $n and
      "\(.width)x\(.height)" == $w)) | "\(.name) \(.width)x\(.height)@\(.refreshRate * 100 | round / 100) \(.x)x\(.y) \(.scale)""#;

/// monitor_add_window: its window, by its class (finding 429).
pub const CLASS_ADDR_JQ: &str = r#"first(.[] | select(.initialClass == $c)) | .address"#;

/// desk_windows: a window of that address is there.
pub const ANY_ADDR_JQ: &str = r#"any(.[]; .address == $a)"#;

/// desk_windows: the main window.
pub const DESK_MAIN_JQ: &str = r#"first(.[] | select(.pid == $p and .workspace.name == $ws and (.address as $a | $k | index($a) | not))) | "main \(.address)" // empty"#;

/// desk_relay: the main window's monitor.
pub const WIN_MONITOR_JQ: &str = r#"first(.[] | select(.address == $a)) | .monitor"#;

/// desk_relay: that monitor.
pub const MON_BY_ID_JQ: &str = r#"first(.[] | select(.id == $id)) // empty"#;

/// desk_relay: the focused monitor.
pub const MON_FOCUSED_JQ: &str = r#"first(.[] | select(.focused)) // empty"#;

/// desk_relay: the host's outer gap.
pub const GAPS_JQ: &str = r#"(.css // (.int | tostring)) | split(" ")[0] | tonumber? // empty"#;

/// desk_relay: the monitor's free area (after JQ_MON).
pub const DESK_AREA_JQ: &str = r#"(.reserved // [0, 0, 0, 0]) as $r |
    "\(.x) \(.y) \(.x + $r[0] + $g) \(.y + $r[1] + $g) \((lw | floor) - $r[0] - $r[2] - 2 * $g) \((lh | floor) - $r[1] - $r[3] - 2 * $g)""#;

/// desk_relay: the picture's scale as a percentage.
pub const DESK_PCT_AWK: &str = r#"BEGIN { printf "%d%%", k * 100 + 0.5 }"#;

/// desk_relay: whether the main window floats.
pub const WIN_FLOATING_JQ: &str = r#"first(.[] | select(.address == $a)) | .floating"#;

/// desk_relay: the main screen's size (after main_screen).
pub const WH_JQ: &str = r#" | "\(.width)x\(.height)""#;

/// desk_picture: the layout as a picture on the host (#174).
pub const DESK_PICTURE_AWK: &str = r#"NR == 1 { aw = $1; ah = $2; g = $3; next }
    NF == 5 { n++; nm[n] = $1; x[n] = $2; y[n] = $3; w[n] = $4; h[n] = $5 }
    END {
      if (!n) exit 1
      minx = x[1]; miny = y[1]; maxx = x[1] + w[1]; maxy = y[1] + h[1]
      for (i = 1; i <= n; i++) {
        if (x[i] < minx) minx = x[i]; if (y[i] < miny) miny = y[i]
        if (x[i] + w[i] > maxx) maxx = x[i] + w[i]; if (y[i] + h[i] > maxy) maxy = y[i] + h[i]
      }
      # Each window is GAP further right per left edge left of its own, and down per top edge above.
      for (i = 1; i <= n; i++) {
        cx = 1; cy = 1
        for (j = 1; j < i; j++) { if (x[j] == x[i]) cx = 0; if (y[j] == y[i]) cy = 0 }
        if (cx) cols++
        if (cy) rows++
        rx[i] = 0; ry[i] = 0
      }
      for (i = 1; i <= n; i++) {
        split("", sx); split("", sy)
        for (j = 1; j <= n; j++) {
          if (x[j] < x[i] && !(x[j] in sx)) { sx[x[j]] = 1; rx[i]++ }
          if (y[j] < y[i] && !(y[j] in sy)) { sy[y[j]] = 1; ry[i]++ }
        }
      }
      fx = (aw - g * (cols - 1)) / (maxx - minx); fy = (ah - g * (rows - 1)) / (maxy - miny)
      f = fx < fy ? fx : fy
      if (f > 1) f = 1
      k = int(f * 120 + 1e-6); fitted = 1
      if (k < 30) { k = 30; fitted = 0 }
      for (j = k; j >= k - 6 && j >= 30; j--) {
        ok = 1
        for (i = 1; i <= n; i++) if ((w[i] * j) % 120 || (h[i] * j) % 120) ok = 0
        if (ok) { k = j; break }
      }
      f = k / 120
      pw = 0; ph = 0
      for (i = 1; i <= n; i++) {
        X[i] = int((x[i] - minx) * f) + g * rx[i]; Y[i] = int((y[i] - miny) * f) + g * ry[i]
        W[i] = int(w[i] * f + 0.5); H[i] = int(h[i] * f + 0.5)
        if (X[i] + W[i] > pw) pw = X[i] + W[i]
        if (Y[i] + H[i] > ph) ph = Y[i] + H[i]
      }
      ox = aw > pw ? int((aw - pw) / 2) : 0; oy = ah > ph ? int((ah - ph) / 2) : 0
      printf "%.5g %d\n", f, fitted
      for (i = 1; i <= n; i++) print nm[i], X[i] + ox, Y[i] + oy, W[i], H[i]
    }"#;

/// monitor_add_window: its line.
pub const MON_LINE_JQ: &str = r#"first(.[] | select(.name == $n)) | "\(.name) \(.width)x\(.height)@\(.refreshRate * 100 | round / 100) \(.x)x\(.y) \(.scale)""#;

/// cost_lines: the aquamarine.
pub const COST_AQ_JQ: &str = r#""aquamarine: " + (if .kind == "private" then "a private build, " else "the system\u0027s, " end)
    + .version + (if .fixed == false then ", without the fix for nested Wayland outputs" else "" end)
    + (if .main_checkout then " (" + .main_checkout + ", the main checkout\u0027s)" else "" end)"#;

/// render_line: the box's GPU (finding 237).
pub const RENDER_LINE_JQ: &str = r#""GPU: \(.driver) \(.pci) (\(.node | sub(".*/"; "")))"
    + (if .fallback then ", a fallback: gpu \(.wanted // "(the setting)") was not usable"
      + (if (.why // "") != "" then " (\(.why))" else "" end) else "" end)"#;

/// cost_lines: apps on a software renderer (#204).
pub const COST_APPS_JQ: &str = r#"select(.software == true) | "apps: \(.renderer | sub(" \\(.*"; "")), in software: their drawing is CPU time, not in the GPU figures""#;

/// gpu's snap: a line per DRM client and engine.
pub const GPU_SNAP_AWK: &str = r#"
        FNR == 1 { flush() }
        /^drm-pdev:/ { pdev = $2 } /^drm-client-id:/ { cid = $2 }
        /^drm-engine-/ && !/^drm-engine-capacity-/ { e = $1; sub(/^drm-engine-/, "", e); sub(/:$/, "", e); eng[e] = $2 }
        function flush(  k) { if (cid != "") for (k in eng) print pdev "/" cid, k, eng[k], pid, comm; pdev = cid = ""; delete eng }
        END { flush() }"#;

/// gpu: the window's figures from two snaps.
pub const GPU_DIFF_AWK: &str = r#"
      FNR == NR { a[$1 " " $2] = $3; next }
      !(($1 " " $2) in seen) { seen[$1 " " $2]; if (!($1 in who)) who[$1] = $5
        d = $3 - a[$1 " " $2]; if (d < 0) d = 0; t[who[$1]] += d; te[who[$1] " " $2] += d }
      END { for (n in t) printf "proc %s %.1f\n", n, t[n] / wall * 100
            for (k in te) printf "engine %s %.1f\n", k, te[k] / wall * 100 }"#;

/// gpu on NVIDIA: whole seconds.
pub const NV_SECS_AWK: &str = r#"BEGIN { n = int(s); if (n < s) n++; print n }"#;

/// gpu --json.
pub const GPU_JSON_JQ: &str = r#"
      [inputs | split(" ")] as $r |
      { box: $box, output: ($screen | split(" ")[0]), mode: ($screen | split(" ")[1]), seconds: $secs, aquamarine: $aq, render: $render,
        source: (if $via == "nvidia" then "nvidia-smi pmon" else "drm fdinfo" end),
        client_render: $cr,
        percent: ([$r[] | select(.[0] == "proc") | {(.[1]): (.[2] | tonumber)}] | add // {}),
        engines: (reduce ($r[] | select(.[0] == "engine")) as $e ({}; .[$e[1]][$e[2]] = ($e[3] | tonumber))) }"#;

/// pmon_figures: NVIDIA's own sampling (finding 295).
pub const PMON_AWK: &str = r#"
    BEGIN { while ((getline l < pidcomm) > 0) { split(l, f, " "); comm[f[1]] = f[2] } }
    /^#/ { if (!ncol) for (i = 2; i <= NF; i++) if ($i != "command") col[++ncol] = $i; next }
    NF >= 3 && ncol {
      key = sprintf("%08d %012d", $1, $2)
      if (prev != "" && key <= prev) sample++
      prev = key
      if (sample == 0 || !($2 in comm)) next
      if (sample > n) n = sample
      seen[comm[$2]]
      for (i = 4; i <= ncol && i <= NF; i++) {
        e = col[i]; if (e == "mem" || $i == "-") continue
        t[comm[$2]] += $i; te[comm[$2] " " e] += $i
      }
    }
    END { for (c in seen) printf "proc %s %.1f\n", c, (n ? t[c] / n : 0)
          for (k in te) printf "engine %s %.1f\n", k, te[k] / n }"#;

/// gpu_rows: a row each.
pub const GPU_ROWS_AWK: &str = r#"{ printf "  %-20s %5.1f%%\n", $2, $3 }"#;

/// cpu --pid: there in the first sample.
pub const CPU_PID_AWK: &str = r#"!/^[St] / && ($6 == p || $1 == p) { f = 1 } END { exit !f }"#;

/// cpu_report: the samples as cpu's figures.
pub const CPU_REPORT_AWK: &str = r#"
    /^S / { t[++k] = $2; next }
    /^t / { key = $2 " " $3 " " $4 " " $5
      if (!(key in tfirst)) { tfirst[key] = k; tbase[key] = k == 1 ? $6 : 0; tkeys[++tn] = key; tpar[key] = $2 " " $3 }
      tlast[key] = k; ttk[key] = $6; tbt[key] = $7; tnm[key] = $8; next }
    { key = $1 " " $2
      if (!(key in first)) { first[key] = k; base[key] = k == 1 ? $3 : 0; keys[++n] = key }
      last[key] = k; tk[key] = $3; rss[key] = $4; pss[key] = $5; bp[key] = $6; nm[key] = $7 }
    END {
      wall = k > 1 ? (t[k] - t[1]) / 1e9 : 0
      sorted = "sort -k1,1rn -k2,2n -k3,3n -k4,4rn -k5,5n | cut -d\" \" -f6-"
      if (want != "") {   # the box numbering first, as gdb --pid
        for (i = 1; i <= n; i++) if (first[keys[i]] == 1 && bp[keys[i]] == want) only = keys[i]
        if (only == "") for (i = 1; i <= n; i++) if (first[keys[i]] == 1 && keys[i] ~ ("^" want " ")) only = keys[i]
      }
      for (i = 1; i <= n; i++) {
        key = keys[i]
        if (want != "" && key != only) continue
        if (app != "" && nm[key] != app) continue
        c = wall > 0 ? (tk[key] - base[key]) / clk / wall * 100 : 0
        if (c < 0) c = 0
        started = first[key] > 1; exited = last[key] < k
        # Sorted on a prefix (cut off): the CPU and order of the process, then its threads by CPU.
        printf "%.1f %d 0 0 0 p %s %s %s %.1f %d %s %d %d\n", c, i, substr(key, 1, index(key, " ") - 1), bp[key], nm[key], c,
          rss[key], pss[key], started, exited | sorted
        for (j = 1; j <= tn; j++) {
          tq = tkeys[j]; if (tpar[tq] != key) continue
          d = wall > 0 ? (ttk[tq] - tbase[tq]) / clk / wall * 100 : 0
          if (d < 0) d = 0
          split(tq, f, " ")
          printf "%.1f %d 1 %.1f %d t %s %s %s %.1f %d %d\n", c, i, d, j, f[3], tbt[tq], tnm[tq], d, (tfirst[tq] > 1), (tlast[tq] < k) | sorted
        }
        tc += c; tr += rss[key]; if (pss[key] != "-") { tp += pss[key]; np++ }
      }
      close(sorted)
      printf "total %.1f %d %s\n", tc, tr, np ? tp : "-"
    }"#;

/// cpu --json.
pub const CPU_JSON_JQ: &str = r#"
      def n: if . == "-" then null else tonumber end;
      [inputs | split(" ")] as $r |
      reduce ($r[] | select(.[0] == "p" or .[0] == "t")) as $x ([];
        if $x[0] == "p" then
          . + [{pid: ($x[2] | tonumber), host_pid: ($x[1] | tonumber), name: $x[3], cpu: ($x[4] | tonumber),
            rss_kb: ($x[5] | tonumber), pss_kb: ($x[6] | n), started: ($x[7] == "1"), exited: ($x[8] == "1")}
            + (if $threads == 1 then {threads: []} else {} end)]
        else .[length - 1].threads += [{tid: ($x[2] | tonumber), host_tid: ($x[1] | tonumber), name: $x[3], cpu: ($x[4] | tonumber),
          started: ($x[5] == "1"), exited: ($x[6] == "1")}] end) as $p |
      first($r[] | select(.[0] == "total")) as $t |
      { box: $box, output: ($screen | split(" ")[0]), mode: ($screen | split(" ")[1]), seconds: $secs, settle: $settle,
        cores: $cores, aquamarine: $aq, render: $render, client_render: $cr,
        percent: (reduce $p[] as $x ({}; .[$x.name] += $x.cpu) | map_values(. * 10 | round / 10)),
        processes: $p, total: {cpu: ($t[1] | tonumber), rss_kb: ($t[2] | tonumber), pss_kb: ($t[3] | n)} }"#;

/// cpu_table: cpu's table.
pub const CPU_TABLE_AWK: &str = r#"function mem(k) { if (k == "-") return "-"; return k >= 1048576 ? sprintf("%.2fG", k / 1048576) : sprintf("%.1fM", k / 1024) }
    BEGIN { printf "  %-20s %8s %7s %9s %9s\n", "PROCESS", "PID", "CPU", "RSS", "PSS" }
    $1 == "p" { note = $8 && $9 ? "  (started and exited in the window)" : $8 ? "  (started in the window)" : $9 ? "  (exited in the window)" : ""
      printf "  %-20s %8s %6.1f%% %9s %9s%s\n", $4, $3, $5, mem($6), mem($7), note; rows++ }
    $1 == "t" { note = $6 && $7 ? "  (started and exited in the window)" : $6 ? "  (started in the window)" : $7 ? "  (exited in the window)" : ""
      printf "    %-18s %8s %6.1f%%%s\n", $4, $3, $5, note }
    $1 == "total" { if (!rows) print "  (no processes)"
      printf "  %-20s %8s %6.1f%% %9s %9s\n", "total", "", $2, mem($3), mem($4) }"#;

/// audio: ffmpeg's levels as a summary and every window's.
pub const AUDIO_AWK: &str = r#"
    function db(v) { return (v ~ /^-?[0-9.]+$/) ? v + 0 : "x" }
    function js(v) { return v == "x" ? "null" : sprintf("%.1f", v) }
    /^frame:/ { n++; pk[n] = "x"; rm[n] = "x"; next }
    /Overall\.Peak_level=/ { sub(/.*=/, ""); pk[n] = db($0); next }
    /Overall\.RMS_level=/ { sub(/.*=/, ""); rm[n] = db($0); next }
    END {
      # -t goes by the pulse input stamps, which let a window or two more through: SECONDS worth.
      m = int(secs * 10 + 0.999); if (n > m) n = m
      lv = ""; sp = ""; on = 0; heard = 0; peak = "x"; pw = 0
      for (i = 1; i <= n; i++) {
        t = (i - 1) / 10
        lv = lv (i > 1 ? "," : "") sprintf("{\"t\":%.1f,\"peak_db\":%s,\"rms_db\":%s}", t, js(pk[i]), js(rm[i]))
        loud = pk[i] != "x" && pk[i] > floor
        if (loud) { heard++; if (peak == "x" || pk[i] > peak) peak = pk[i]; if (rm[i] != "x") pw += 10 ^ (rm[i] / 10) }
        if (loud && !on) { from = t; on = 1 }
        if (!loud && on) { sp = sp (sp == "" ? "" : ",") sprintf("[%.1f,%.1f]", from, t); on = 0 }
      }
      if (on) sp = sp (sp == "" ? "" : ",") sprintf("[%.1f,%.1f]", from, n / 10)
      rms = heard && pw > 0 ? 10 * log(pw / heard) / log(10) : "x"
      printf "{\"box\":\"%s\",\"seconds\":%s,\"sound\":%s,\"floor_db\":%d,\"sound_s\":%.1f,\"peak_db\":%s,\"rms_db\":%s,\"spans\":[%s],\"levels\":[%s]}\n",
        box, secs + 0, heard ? "true" : "false", floor, heard / 10, js(peak), js(rms), sp, lv
    }"#;

/// tcp_listeners: the listening TCP sockets of a /proc net dir.
pub const TCP_LISTENERS_AWK: &str = r#"function hex(s,  i, n) { n = 0; s = toupper(s)
         for (i = 1; i <= length(s); i++) n = n * 16 + index("0123456789ABCDEF", substr(s, i, 1)) - 1; return n }
       function v4(h) { return hex(substr(h, 7, 2)) "." hex(substr(h, 5, 2)) "." hex(substr(h, 3, 2)) "." hex(substr(h, 1, 2)) }
       FNR > 1 && $4 == "0A" { split($2, a, ":"); h = toupper(a[1])
         if (length(h) == 8) addr = v4(h)
         else if (h == "00000000000000000000000000000000") addr = "::"
         else if (h == "00000000000000000000000001000000") addr = "::1"
         else if (substr(h, 1, 24) == "0000000000000000FFFF0000") addr = "::ffff:" v4(substr(h, 25, 8))
         else addr = h
         print hex(a[2]), addr, $8, $10 }"#;

/// ports --json: a port each.
pub const PORTS_JSON_JQ: &str = r#"
        { box: $box, port: $port, addresses: ($addr | split(",")), process: $proc,
          host: ({ state: $state }
            + (if $state == "this" or $state == "this-resets" or $state == "shared" then { boxes: ($boxes | split(" ")) } else {} end)
            + (if $state == "shared" then { resets: ($resets | split(" ")) } else {} end)
            + (if $state == "host" and $who != "" then { pid: ($who | tonumber), process: $hcomm } else {} end)
            + (if $state == "other-user" and $who != "" then { uid: ($who | tonumber) } else {} end)) }"#;

/// clip: the item onto the box's clipboard (wl-copy forks to serve it, in a session of its own).
pub const CLIP_BOX_COPY: &str = r#"e=$(/usr/bin/mktemp) || exit 1
/usr/bin/timeout 10 /usr/bin/setsid -w /usr/bin/wl-copy --type "$1" ${2:+"$2"} >/dev/null 2>"$e"; rc=$?
/usr/bin/tail -n 1 "$e" >&2; /usr/bin/rm -f "$e"; exit $rc"#;

/// clip --from-box: the box's clipboard's types (3: nothing copied).
pub const CLIP_BOX_TYPES: &str = r#"o=$(/usr/bin/timeout 10 /usr/bin/wl-paste --list-types 2>&1) && { printf "%s\n" "$o"; exit 0; }
case $o in *"Nothing is copied"*) exit 3 ;; esac; exit 1"#;

/// monitor add: the box's aquamarine, which must have +appid (finding 429).
pub const AQ_VERSION_JQ: &str = r#".aquamarine.version // "?""#;
/// audio's words (bash's program had the quotes around the box's name as '"'"').
pub const AUDIO_SAY_JQ: &str = concat!(
    r#"if .sound then
      "sound for \(.sound_s)s of \(.seconds)s: peak \(.peak_db) dBFS, RMS \(.rms_db) dBFS while it sounded; at "
      + ([.spans[:10][] | "\(.[0])-\(.[1])s"] | join(", ")) + (if (.spans | length) > 10 then ", ..." else "" end)
    else "silence for \(.seconds)s: nothing played to the speaker of box "#,
    "'",
    r#"\(.box)"#,
    "'",
    r#"" end"#
);
/// agent_caller (AGENT_MARKS): what gives an agent's process away, in its environment.
pub const AGENT_MARKS: &str = r#"^(CLAUDECODE|CLAUDE_CODE_SESSION_ID|CLAUDE_PID|CODEX_THREAD_ID|CODEX_SANDBOX|OPENCODE|AI_AGENT|OMABOX_AGENT_PID)=|^OMABOX_SESSION=.|^(WAYLAND_DISPLAY|HYPRLAND_INSTANCE_SIGNATURE)=omabox-guard$|^PATH=([^:]*:)*[^:]*/share/guard/?(:|$)"#;
