#!/usr/bin/env bash
# Records the hero demo (website/public/demo.cast) by driving `oy` over the
# fixture repo. pilotty sends full-fidelity input (keys, ctrl-chords, mouse) and
# asciinema captures the cast with real timing. See DEMO_RECORDING.md.
#
# Prereqs: pilotty (npm i -g pilotty), asciinema (cargo install asciinema), node.
# Run website/scripts/demo/setup-repo.sh first.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
OY="${OYO_BIN:-$ROOT/target/release/oy}"
FIXTURE="${OYO_DEMO_FIXTURE:-/tmp/oyo/demo}"
CFG="$HERE/demo.config.toml"
OUT="${1:-$ROOT/website/public/demo.cast}"
THEME="${OYO_DEMO_THEME:-evergarden-winter}"
MODE="${OYO_DEMO_MODE:-dark}"
COLS=112; ROWS=34
SESS=oyodemo

K(){ pilotty key "$@" -s "$SESS" >/dev/null 2>&1; }
T(){ pilotty type "$1" -s "$SESS" >/dev/null 2>&1; }
C(){ pilotty click "$1" "$2" -s "$SESS" >/dev/null 2>&1; }   # ROW COL (0-based row)
W(){ pilotty wait-for "$1" -t "${2:-5000}" -s "$SESS" >/dev/null 2>&1; }
P(){ pilotty wait-for "ZZ_NOPE_$RANDOM" -t "$1" -s "$SESS" >/dev/null 2>&1; }  # pause ms

pilotty kill -s "$SESS" 2>/dev/null; rm -f "$OUT"
pilotty spawn --name "$SESS" --cwd "$FIXTURE" -- \
  asciinema rec --overwrite --window-size "${COLS}x${ROWS}" -f asciicast-v2 --idle-time-limit 2 \
  -c "$OY --config $CFG --no-review-persist --theme-name $THEME --theme-mode $MODE" "$OUT" >/dev/null 2>&1
pilotty resize "$COLS" "$ROWS" -s "$SESS" >/dev/null 2>&1
W "Formats" 12000; P 1000

# 1. nav to the code file; unified word-diff and hunk
K "] ] ] ] ]" --delay 320; W "parser.rs" 4000; P 700
K "j j j" --delay 300; K l; P 1000

# 2. inline review comment
K m; W "cancel" 3000; T "LGTM, ship it!"; K Ctrl+S; W "delete" 3000; P 1600

# 3. step-through and autoplay morph
K s; P 500; K Space; P 1700; K Space; P 300; K s; P 700

# 4. split view
K Tab; W "SPLIT" 3000; K "j j" --delay 300; P 900

# 5. blame view
K Tab; W "BLAME" 3000; P 1500

# 6. preview walk (reverse file order): image to csv to yaml to json to markdown
K Tab; W "PREVIEW" 3000; P 1100
K "["; W "logo.png" 3000; P 1800
K "["; W "data.csv" 3000; P 1600
K "["; W "config.yaml" 3000; P 1500
K "["; W "api.json" 3000; P 1500
K "["; W "README" 3000; P 1600

# 7. tabs (mouse, 0-based rows): new-tab button to file search to a new tab to a different view to switch back
C 1 42; P 800
T "parser"; P 500; K Enter; W "parser.rs" 3000; P 1000
K Tab; W "SPLIT" 3000; P 1400
C 1 33; P 1500

# 8. commit dashboard (oy view)
K Ctrl+R; W "Working tree" 4000; P 1500
K j; P 500; K j; P 800
K q; P 700
K q; P 700; K q; P 500; K q      # quit oy so asciinema flushes
W "Recorded" 8000

pilotty kill -s "$SESS" 2>/dev/null
node "$HERE/trim.mjs" "$OUT"
echo "RECORDED: $OUT"
