#!/usr/bin/env bash
# Records a small "two modes" clip. Usage: record-mini.sh <out.cast> <scroll|step>
# Same pilotty+asciinema pipeline as record.sh, but tiny geometry, no sidebar.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
OY="${OYO_BIN:-$ROOT/target/release/oy}"
FIXTURE="${OYO_DEMO_FIXTURE:-/tmp/oyo/demo}"
CFG="$HERE/mini.config.toml"
OUT="$1"
MODE="$2"
THEME="${OYO_DEMO_THEME:-evergarden-winter}"
TMODE="${OYO_DEMO_MODE:-dark}"
COLS=64; ROWS=14
SESS=oyomini
STEPFLAG=""; [ "$MODE" = step ] && STEPFLAG="--step"

K(){ pilotty key "$@" -s "$SESS" >/dev/null 2>&1; }
W(){ pilotty wait-for "$1" -t "${2:-8000}" -s "$SESS" >/dev/null 2>&1; }
P(){ pilotty wait-for "ZZ_$RANDOM" -t "$1" -s "$SESS" >/dev/null 2>&1; }

pilotty kill -s "$SESS" 2>/dev/null; rm -f "$OUT"
pilotty spawn --name "$SESS" --cwd "$FIXTURE" -- \
  asciinema rec --overwrite --window-size "${COLS}x${ROWS}" -f asciicast-v2 --idle-time-limit 2 \
  -c "$OY $STEPFLAG --config $CFG --no-review-persist --theme-name $THEME --theme-mode $TMODE parser.rs" "$OUT" >/dev/null 2>&1
pilotty resize "$COLS" "$ROWS" -s "$SESS" >/dev/null 2>&1
W "parser.rs" 10000; P 800             # diff of the code file
if [ "$MODE" = step ]; then
  for _ in 1 2 3 4; do K j; P 850; done   # step through: morph plays each step
else
  for _ in 1 2 3 4 5 6; do K j; P 380; done   # scroll through the diff
fi
P 600
K q; W "Recorded" 6000
pilotty kill -s "$SESS" 2>/dev/null
node "$HERE/trim.mjs" "$OUT"
echo "RECORDED: $OUT ($MODE ${COLS}x${ROWS})"
