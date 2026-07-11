#!/usr/bin/env bash
# Regenerate the three review-showcase card demos into website/public/*.cast:
#   review-tool.cast   card 1  "Diff and review"     — the oy TUI, diff + comment threads
#   review-team.cast   card 2  "Sync with your team" — CLI: git branch, oy review pull, status
#   review-agent.cast  card 3  "Work with your agent"— CLI (Claude replies + resolves) -> TUI reveal
#
# Run from website/:  npm run generate-review-demos
#
# Prerequisites (these drive REAL `oy review` against a dogfood PR):
#   - pilotty   : npm i -g pilotty
#   - asciinema : cargo install asciinema   (v2/v3; we force asciicast-v2)
#   - gh        : authenticated (`gh auth status`)
#   - a dogfood git repo with an open PR #1 that has exactly the 4 curated
#     comments:  #1 README.md R3 · #2/#3 src/app.txt · #4 PR-level.
#     Path via OYO_REVIEW_DOGFOOD (default below).
#
# Note: the demos pull/sync live comments, so they aren't hermetic — they reset
# the dogfood to its clean 4-comment state each run and clean up after.
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
PUBLIC="$ROOT/website/public"
DOG="${OYO_REVIEW_DOGFOOD:-/tmp/oyo-review-dogfood-1783482731}"
OY="${OYO_BIN:-oy}"
RANGE="${OYO_REVIEW_RANGE:-main...feature/review-sync}"
# Recorded in both dark and light. THEME/TMODE/SUFFIX are set per pass by gen_all
# below; the "-light" suffix mirrors the hero (demo.cast / demo-light.cast).
DARK_THEME="${OYO_DEMO_DARK_THEME:-evergarden-winter}"
LIGHT_THEME="${OYO_DEMO_LIGHT_THEME:-evergarden-winter}"
THEME="$DARK_THEME"; TMODE="dark"; SUFFIX=""
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
export GIT_PAGER=cat _ZO_DOCTOR=0

need(){ command -v "$1" >/dev/null 2>&1 || { echo "error: '$1' not found — $2" >&2; exit 1; }; }
need pilotty "npm i -g pilotty"; need asciinema "cargo install asciinema"
need "$OY" "install oyo"; need python3 "your package manager"
[ -d "$DOG/.git" ] || { echo "error: dogfood repo not found at $DOG (set OYO_REVIEW_DOGFOOD)" >&2; exit 1; }

VT="$HERE/vt-snap.mjs"   # optional headless verifier (from the tui-demo skill); ok if absent

# shared oy config: no quit confirmation, sidebar hidden so the diff fills the frame
cat > "$WORK/tui.toml" <<'TOML'
[ui]
confirm_quit = false
[files]
panel_visible = false
TOML

reset_dog(){ (
  cd "$DOG"
  "$OY" review pull >/dev/null 2>&1 || true
  for id in $("$OY" review status --json 2>/dev/null | python3 -c 'import sys,json
try:
    d=json.load(sys.stdin); print(" ".join(str(c["id"]) for c in d["comments"] if c["id"]>4))
except Exception: pass'); do "$OY" review comment rm "$id" --yes >/dev/null 2>&1 || true; done
  "$OY" review comment unresolve 1 >/dev/null 2>&1 || true
  "$OY" review comment unresolve 3 >/dev/null 2>&1 || true
); }

# ---------------------------------------------------------------------------
# card 2 — Sync with your team (CLI). 90x20 so the aspect fills the 565x294 box.
# ---------------------------------------------------------------------------
gen_team(){
  cat > "$WORK/team.sh" <<TEAM
export _ZO_DOCTOR=0 GIT_PAGER=cat
cd "$DOG" || exit 1
clear; sleep 0.7
printf '\$ git branch --show-current\n'; sleep 0.45
git branch --show-current; sleep 1.2
printf '\$ oy review pull\n'; sleep 0.45
$OY --theme-name $THEME --theme-mode $TMODE review pull; sleep 1.8
printf '\$ oy review status\n'; sleep 0.45
$OY --theme-name $THEME --theme-mode $TMODE review status; sleep 3.4
printf '\033[0m'
TEAM
  ( cd "$DOG" && "$OY" review pull >/dev/null 2>&1 || true )   # warm gh
  local RAW="$WORK/team_raw.cast" OUT="$PUBLIC/review-team${SUFFIX}.cast"
  asciinema rec --overwrite --window-size 90x20 -f asciicast-v2 --idle-time-limit 4 \
    -c "bash $WORK/team.sh" "$RAW" >/dev/null 2>&1
  python3 - "$RAW" "$OUT" <<'PY'
import sys, json
L=open(sys.argv[1]).read().splitlines(); h=L[0]; evs=[json.loads(x) for x in L[1:] if x.strip()]
last=max(i for i,e in enumerate(evs) if 'Review comments' in e[2] or 'Pulled' in e[2])
kept=evs[:last+1]+[[evs[-1][0],'o','\x1b[0m']]
open(sys.argv[2],'w').write(h+'\n'+''.join(json.dumps(e)+'\n' for e in kept))
PY
  echo "  review-team${SUFFIX}.cast"
}

# ---------------------------------------------------------------------------
# card 1 — Diff and review (oy TUI). Drives } to walk the comment threads.
# ---------------------------------------------------------------------------
gen_tool(){
  local RAW="$WORK/tool_raw.cast" OUT="$PUBLIC/review-tool${SUFFIX}.cast" S=oyrevtui
  pilotty kill -s "$S" 2>/dev/null || true; rm -f "$RAW"
  pilotty spawn --name "$S" --cwd "$DOG" -- \
    asciinema rec --overwrite --window-size 80x18 -f asciicast-v2 --idle-time-limit 2 \
    -c "$OY --range $RANGE --config $WORK/tui.toml --theme-name $THEME --theme-mode $TMODE" "$RAW" >/dev/null 2>&1
  pilotty resize 80 18 -s "$S" >/dev/null 2>&1
  # Walk the comment threads (} = next comment) so the card actually reviews:
  # opens on the README thread, then steps across the others (src/app.txt, the
  # PR-level note). pilotty keys reach oy fine through asciinema — same as the
  # hero — so this stays deterministic (the 4 curated comments cycle in order).
  # `seek` presses } until the target thread is focused — absorbs the one-off
  # startup key-eat and keeps the landing deterministic. The file panel is hidden,
  # so a filename only appears once its comment is focused, making the wait exact.
  hold(){ pilotty wait-for "ZZ_$RANDOM" -t "$1" -s "$S" >/dev/null 2>&1 || true; }
  seek(){ local n=0; while [ $n -lt 5 ]; do
    pilotty wait-for "$1" -t 200 -s "$S" >/dev/null 2>&1 && return 0
    pilotty key "}" -s "$S" >/dev/null 2>&1; hold 650; n=$((n+1)); done; }
  pilotty wait-for "README.md R3" -t 12000 -s "$S" >/dev/null 2>&1 || true; hold 2900
  seek "src/app.txt"; hold 2900                          # a code-file comment
  pilotty key "}" -s "$S" >/dev/null 2>&1; hold 2600     # its sibling thread
  pilotty key q -s "$S" >/dev/null 2>&1
  pilotty wait-for "Recorded" -t 6000 -s "$S" >/dev/null 2>&1 || true
  pilotty kill -s "$S" 2>/dev/null || true
  python3 - "$RAW" "$OUT" <<'PY'
import sys, json, re
apc=re.compile(r'\x1b_.*?\x1b\\',re.S); kbd=re.compile(r'\x1b\[[<>=?][0-9;]*u')
esc=re.compile(r'\x1b\[[0-9;?]*[A-Za-z]|\x1b[()][0-9A-B]|\x1b[=>0-9]'); cup=re.compile(r'\x1b\[[0-9;]*[Hf]')
L=open(sys.argv[1]).read().splitlines(); h=L[0]; evs=[json.loads(x) for x in L[1:] if x.strip()]
# strip kitty, drop teardown (alt-screen exit), keep real timeline
clean=[]
for e in evs:
    d=kbd.sub('',apc.sub('',e[2]))
    if '\x1b[?1049l' in d: break
    clean.append([e[0],'o',d])
# start on the painted diff (fold ~2s avatar-loading startup into t=0)
first=next((i for i,e in enumerate(clean) if len(e[2])>500),1)
for e in clean[:first]: e[0]=0.0
sh=clean[first][0]
for e in clean[first:]: e[0]=round(max(0.0,e[0]-sh),3)
# drop idle cursor-blink repaints and stray terminal-query echoes (e.g. a bare
# "143" cursor/DA response that would otherwise flash at the bottom), then hold
# the last thread before looping.
noop=lambda d: bool(d) and not esc.sub('',d).strip() and not cup.search(d)
junk=lambda d: bool(d) and '\x1b' not in d and len(d)<=8
keep=[e for e in clean if not noop(e[2]) and not junk(e[2])]
keep.append([round(keep[-1][0]+2.0,3),'o',''])
open(sys.argv[2],'w').write(h+'\n'+''.join(json.dumps(e)+'\n' for e in keep))
PY
  echo "  review-tool${SUFFIX}.cast"
}

# ---------------------------------------------------------------------------
# card 3 — Work with your agent. Two casts stitched: CLI (Claude replies +
# resolves) then the TUI reveal. Recorded separately because pilotty keys don't
# reach oy through the asciinema+shell wrapper — so the TUI half needs no nav
# (oy opens right on the README comment the agent replied to).
# ---------------------------------------------------------------------------
gen_agent(){
  reset_dog
  cat > "$WORK/agent.sh" <<AGENT
export _ZO_DOCTOR=0 GIT_PAGER=cat
export OYO_REVIEW_AUTHOR_TYPE=agent OYO_REVIEW_AUTHOR_NAME=Claude OYO_REVIEW_AUTHOR_EMAIL=noreply@anthropic.com
cd "$DOG" || exit 1
clear; sleep 0.7
printf '\$ export OYO_REVIEW_AUTHOR_TYPE=agent OYO_REVIEW_AUTHOR_NAME=Claude \\\\\n'
printf '      OYO_REVIEW_AUTHOR_EMAIL=noreply@anthropic.com\n'; sleep 0.9
printf '\$ oy review comment reply 1 \\\\\n'
printf '      --body "Done — added a usage example to the README."\n'; sleep 0.5
$OY review comment reply 1 --body "Done — added a usage example to the README."; sleep 1.4
printf '\$ oy review comment resolve 1\n'; sleep 0.5
$OY review comment resolve 1; sleep 1.4
printf '\$ oy --range $RANGE\n'; sleep 0.9
AGENT
  local CLI="$WORK/agent_cli.cast" TUI="$WORK/agent_tui.cast" OUT="$PUBLIC/review-agent${SUFFIX}.cast" S=agt
  asciinema rec --overwrite --window-size 80x18 -f asciicast-v2 --idle-time-limit 3 \
    -c "bash $WORK/agent.sh" "$CLI" >/dev/null 2>&1
  pilotty kill -s "$S" 2>/dev/null || true; rm -f "$TUI"
  pilotty spawn --name "$S" --cwd "$DOG" -- \
    asciinema rec --overwrite --window-size 80x18 -f asciicast-v2 --idle-time-limit 2 \
    -c "$OY --range $RANGE --config $WORK/tui.toml --theme-name $THEME --theme-mode $TMODE" "$TUI" >/dev/null 2>&1
  pilotty resize 80 18 -s "$S" >/dev/null 2>&1
  pilotty wait-for "Claude" -t 18000 -s "$S" >/dev/null 2>&1 || pilotty wait-for "README.md R3" -t 6000 -s "$S" >/dev/null 2>&1 || true
  pilotty wait-for "ZZ_$RANDOM" -t 2600 -s "$S" >/dev/null 2>&1 || true
  pilotty key q -s "$S" >/dev/null 2>&1
  pilotty wait-for "Recorded" -t 6000 -s "$S" >/dev/null 2>&1 || true
  pilotty kill -s "$S" 2>/dev/null || true
  python3 - "$CLI" "$TUI" "$OUT" <<'PY'
import sys, json, re
apc=re.compile(r'\x1b_.*?\x1b\\',re.S); kbd=re.compile(r'\x1b\[[<>=?][0-9;]*u')
def load(p): L=open(p).read().splitlines(); return L[0],[json.loads(x) for x in L[1:] if x.strip()]
h,cli=load(sys.argv[1]); _,tui=load(sys.argv[2]); T1=cli[-1][0]; tc=[]
for e in tui:
    d=kbd.sub('',apc.sub('',e[2]))
    if '\x1b[?1049l' in d: break
    if d and '\x1b' not in d and len(d)<=8: continue   # drop stray query echoes (e.g. "143")
    tc.append([e[0],'o',d])
first=next((i for i,e in enumerate(tc) if len(e[2])>500),1); ct=tc[first][0]
# Clear the screen right before the TUI's first paint so the CLI's last line
# (the `oy` command) doesn't linger on the reveal's top row at the stitch seam.
new=[]
for i,e in enumerate(tc):
    t=T1 if i<first else round(T1+0.3+(e[0]-ct),3)
    d=('\x1b[2J\x1b[3J\x1b[H'+e[2]) if i==first else e[2]
    new.append([t,'o',d])
allev=cli+new; allev.append([round(allev[-1][0]+1.6,3),'o',''])
open(sys.argv[3],'w').write(h+'\n'+''.join(json.dumps(e)+'\n' for e in allev))
PY
  reset_dog
  echo "  review-agent${SUFFIX}.cast"
}

# Record every card in both themes so the page can swap per prefers-color-scheme,
# mirroring the hero's demo.cast / demo-light.cast pair.
gen_all(){   # $1=theme  $2=mode  $3=suffix
  THEME="$1"; TMODE="$2"; SUFFIX="$3"
  echo "==> [$2] recording review card demos into $PUBLIC"
  reset_dog
  gen_team
  gen_tool
  gen_agent
}

gen_all "$DARK_THEME"  dark  ""
gen_all "$LIGHT_THEME" light "-light"
echo "done."
