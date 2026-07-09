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
case "$OUT" in /*) ;; *) OUT="$PWD/$OUT" ;; esac  # asciinema runs cwd=FIXTURE; keep OUT absolute
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

# Step ]/[ until the *active file's diff content* ($2) shows — matching the
# filename alone is unreliable (it's always in the sidebar, so the wait passes
# before we've arrived). The per-step hold guards the one-off startup key-eat and
# paces the walk so viewers see each file scroll past, not a blur.
walk(){ local n=0; while [ $n -lt 9 ]; do
  pilotty wait-for "$2" -t 150 -s "$SESS" >/dev/null 2>&1 && return 0
  K "$1"; P 520; n=$((n+1)); done; }

# 1. nav to the code file (parser.rs); show its hunk / word-diff
walk "]" "fn port"; P 700
K "j j j" --delay 300; K l; P 1000

# 2. inline review comment — paced so the editor doesn't just flash: hold the
#    empty box, hold the typed comment so it's readable, then hold the saved card.
K m; W "cancel" 3000; P 800
T "LGTM, ship it!"; P 1500
K Ctrl+S; W "delete" 3000; P 1600

# (Step-through / autoplay is demoed in the site's "Two modes" section, so it's
#  intentionally left out of the hero demo.)

# Select views via the command palette (ctrl-p) rather than Tab-cycling — the
# Tab cycle order depends on step/blame state, which is fragile. The palette's
# "View: X" entries set the mode directly, deterministically.
# Paced so the palette doesn't flash: hold it open, hold the filtered result,
# then hold after it closes — reads as a deliberate pick, not a blink.
view(){ K Ctrl+p; W "View: $1" 3000; P 550; T "View: $1"; P 750; K Enter; }

# 3. split view (still parser.rs)
view Split; W "SPLIT" 3000; K "j j" --delay 300; P 900

# (Blame lives in the History beat below: over the working tree every line is
#  "Uncommitted", so blame only shows real per-line authorship on a committed
#  range — shown there, not here on an untracked image.)

# 4. rich previews — the centerpiece. From parser.rs walk back through each
#    format, holding long enough to read it: image, CSV table, YAML tree, JSON
#    tree, rendered Markdown.
view Preview; W "PREVIEW" 3000; P 1600
K "["; W "logo.png" 3000; P 2500
K "["; W "data.csv" 3000; P 2500
K "["; W "config.yaml" 3000; P 2500
K "["; W "api.json" 3000; P 2500
K "["; W "README" 3000; P 2800

# 6. tabs (mouse, 0-based rows): new-tab button to file search to a new tab to a different view to switch back
C 1 42; P 800
T "parser"; P 500; K Enter; W "parser.rs" 3000; P 1000
view Split; W "SPLIT" 3000; P 1400
C 1 33; P 1500

# 6. History → open a committed range → blame README with real authorship.
#    (Ctrl-R opens History; filter to one commit so the pick is deterministic;
#    Enter opens its diff, which is committed, so blame shows author + date per
#    line instead of the working tree's "Uncommitted".)
K Ctrl+R; W "month ago" 5000; P 1600
K "/"; W "Filter" 3000; T "Document"; W "Document the data" 3000; P 900
K Enter; W "README" 4000; P 1000
view Blame; W "BLAME" 3000; P 2800
K q; P 700; K q; P 700; K q; P 500; K q; K q    # quit blame->diff->history->oy
W "Recorded" 8000

pilotty kill -s "$SESS" 2>/dev/null
node "$HERE/trim.mjs" "$OUT"
echo "RECORDED: $OUT"
