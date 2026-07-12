#!/usr/bin/env bash
# Regenerate every demo asset from scratch and drop it where it's used:
#   - website casts  -> website/public/*.cast   (hero + mode clips, light + dark)
#   - README GIF     -> docs/assets/demo.gif     (rendered from the dark hero cast)
#
# Run from the website/ directory: npm run generate-demos
#
# One-time prerequisites:
#   pilotty    npm i -g pilotty
#   asciinema  cargo install asciinema
#   agg        cargo install --git https://github.com/asciinema/agg
#   gifsicle   optional, shrinks the GIF (apt/brew install gifsicle)
set -eu
# Neutralize any user git pager (riff, delta, less, ...) so git in the fixture
# setup and the recordings never blocks on an interactive pager.
export GIT_PAGER=cat
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
PUBLIC="$ROOT/website/public"
ASSETS="$ROOT/docs/assets"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: '$1' not found. Install it: $2" >&2
    exit 1
  }
}
need pilotty "npm i -g pilotty"
need asciinema "cargo install asciinema"
need agg "cargo install --git https://github.com/asciinema/agg"

# 1. Build the release binary the recordings drive (so casts reflect latest code).
echo "==> building oy (release)"
cargo build --manifest-path "$ROOT/Cargo.toml" --release --bin oy

# 2. Rebuild the throwaway fixture repo.
echo "==> setting up fixture"
bash "$HERE/setup-repo.sh"

# 3. Hero casts. Both modes use evergarden-winter (now has a light variant).
echo "==> recording hero (dark + light)"
OYO_DEMO_THEME=evergarden-winter OYO_DEMO_MODE=dark \
  bash "$HERE/record.sh" "$PUBLIC/demo.cast"
OYO_DEMO_THEME=evergarden-winter OYO_DEMO_MODE=light \
  bash "$HERE/record.sh" "$PUBLIC/demo-light.cast"

# 4. Two-mode clips (scroll / step), light + dark.
echo "==> recording mode clips (dark + light)"
OYO_DEMO_THEME=evergarden-winter OYO_DEMO_MODE=dark \
  bash "$HERE/record-mini.sh" "$PUBLIC/scroll-mode.cast" scroll
OYO_DEMO_THEME=evergarden-winter OYO_DEMO_MODE=dark \
  bash "$HERE/record-mini.sh" "$PUBLIC/step-mode.cast" step
OYO_DEMO_THEME=evergarden-winter OYO_DEMO_MODE=light \
  bash "$HERE/record-mini.sh" "$PUBLIC/scroll-mode-light.cast" scroll
OYO_DEMO_THEME=evergarden-winter OYO_DEMO_MODE=light \
  bash "$HERE/record-mini.sh" "$PUBLIC/step-mode-light.cast" step

# 5. README GIF, rendered from the dark hero cast (the .cast already carries
#    truecolor, so agg reproduces exactly what the website player shows).
#    agg needs a monospace font. Rather than depend on the machine's fontconfig
#    (sparse on nix/devbox) or shipping a font in the repo, fetch JetBrains Mono
#    into a tmp dir and point agg at it with --font-dir, so this reproduces
#    anywhere. Cached across runs. Override with OYO_DEMO_FONT_DIR / _FAMILY.
echo "==> rendering README gif -> docs/assets/demo.gif"
mkdir -p "$ASSETS"
FONT_FAMILY="${OYO_DEMO_FONT_FAMILY:-JetBrains Mono}"
FONT_DIR="${OYO_DEMO_FONT_DIR:-/tmp/oyo/fonts}"
# Higher font size = higher-res GIF (crisper, larger). 24 ~ 2x GitHub's display
# width. Bump OYO_DEMO_FONT_SIZE for more.
FONT_SIZE="${OYO_DEMO_FONT_SIZE:-24}"
if [ -z "${OYO_DEMO_FONT_DIR:-}" ]; then
  need curl "your package manager"
  mkdir -p "$FONT_DIR"
  jbm="https://github.com/JetBrains/JetBrainsMono/raw/master/fonts/ttf"
  for style in Regular Bold Italic BoldItalic; do
    ttf="$FONT_DIR/JetBrainsMono-$style.ttf"
    [ -f "$ttf" ] || { echo "    fetching JetBrainsMono-$style.ttf"; curl -fsSL -o "$ttf" "$jbm/JetBrainsMono-$style.ttf"; }
  done
fi
agg --font-size "$FONT_SIZE" --font-dir "$FONT_DIR" --font-family "$FONT_FAMILY" \
  "$PUBLIC/demo.cast" "$ASSETS/demo.gif"

# Crop agg's padding (1 cell horizontal, 0.5 cell vertical per side) so the
# terminal sits flush in the README. Hero geometry is 112x34 (see record.sh).
# node resolves sharp from the script's own node_modules, regardless of cwd.
echo "==> cropping gif padding (sharp)"
node "$HERE/crop-gif.mjs" "$ASSETS/demo.gif" 112 34

# Optional extra shrink if gifsicle happens to be installed.
if command -v gifsicle >/dev/null 2>&1; then
  echo "==> optimizing gif (gifsicle)"
  gifsicle -O3 --lossy=80 -b "$ASSETS/demo.gif"
fi

echo
echo "done."
echo "  website casts : $PUBLIC/*.cast"
echo "  README gif    : $ASSETS/demo.gif"
