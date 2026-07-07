#!/usr/bin/env bash
# Builds the throwaway fixture repository the hero demo is recorded against.
# Deterministic content and git history so blame, the dashboard and the multi-file
# sidebar all have something real to show. Safe to re-run.
set -eu
# Don't let the user's global git pager (riff, delta, ...) run for this script's
# git commands; some pagers aren't installed and would abort under `set -e`.
export GIT_PAGER=cat
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(git -C "$HERE" rev-parse --show-toplevel)"
SAMPLES="$ROOT/docs/assets"
FIXTURE="${OYO_DEMO_FIXTURE:-/tmp/oyo/demo}"

rm -rf "$FIXTURE"; mkdir -p "$FIXTURE"; cd "$FIXTURE"
git init -q
git config user.email "demo@oyo.dev"
git config user.name "Ada"
git config commit.gpgsign false
commit() { GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" git commit -qm "$2"; }

# ---- commit 1: parser ----
cat > parser.rs <<'RS'
use std::collections::HashMap;

/// Parses `KEY=VALUE` lines into a map.
pub fn parse_env(input: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=').unwrap();
        out.insert(key.to_string(), value.to_string());
    }
    out
}

fn main() {
    let raw = "HOST=localhost\nPORT=8080\n";
    let env = parse_env(raw);
    println!("host = {}", env["HOST"]);
    println!("port = {}", env["PORT"]);
}
RS
cat > README.md <<'MD'
# Oyo preview samples

A tiny project that exercises Oyo's data previews.
MD
git add -A; commit "2026-05-20T10:00:00" "Add config parser"

# ---- commit 2: data samples ----
cp "$SAMPLES/preview.json" api.json
cp "$SAMPLES/preview.yaml" config.yaml
cp "$SAMPLES/preview.csv"  data.csv
git add -A; commit "2026-05-27T14:00:00" "Add JSON, YAML and CSV samples"

# ---- commit 3: docs ----
cat >> README.md <<'MD'

## Formats

- Markdown
- JSON
- YAML
- CSV
MD
git add -A; commit "2026-06-02T09:30:00" "Document the data formats"

# ---- commit 4: parser doc tweak (blame variety) ----
sed -i 's#/// Parses `KEY=VALUE` lines into a map.#/// Parses `KEY=VALUE` config lines into a map.#' parser.rs
git add -A; commit "2026-06-06T16:20:00" "Clarify the parser doc comment"

# ---- working changes (the diff oy shows by default) ----
cat > parser.rs <<'RS'
use std::collections::HashMap;

/// Parses `KEY=VALUE` config lines into a map, skipping blanks and `#` comments.
pub fn parse_env(input: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        out.insert(key.trim().to_string(), value.trim().to_string());
    }
    out
}

fn port(env: &HashMap<String, String>) -> u16 {
    env.get("PORT").and_then(|p| p.parse().ok()).unwrap_or(3000)
}

fn main() {
    let raw = "# service config\nHOST = localhost\nPORT = 8080\n";
    let env = parse_env(raw);
    println!("host = {}", env["HOST"]);
    println!("port = {}", port(&env));
}
RS
cat > README.md <<'MD'
# Oyo preview samples

A tiny project that exercises Oyo's interactive data previews.

## Formats

- Markdown: this file
- JSON: `api.json`
- YAML: `config.yaml`
- CSV: `data.csv`

## Parser

See `parser.rs` for the `KEY=VALUE` config parser. It now skips
blank lines and `#` comments.

| Format | Preview |
| ------ | ------- |
| json   | tree    |
| csv    | table   |
MD
sed -i 's/"version": 1,/"version": 2,/' api.json
sed -i 's/^version: 1$/version: 2/' config.yaml
sed -i 's/sample preview row 1/sample preview row 1 (edited)/' data.csv

# ---- logo.png: everforest gradient (new/untracked -> shows as an added binary
# in the diff, so it's navigable in the sidebar for the image preview) ----
python3 - "$FIXTURE/logo.png" <<'PY'
import sys, zlib, struct
path = sys.argv[1]
W, H = 260, 150
stops = [(0x23,0x2a,0x2e),(0x83,0xc0,0x92),(0xa7,0xc0,0x80),(0xdb,0xbc,0x7f),
         (0xe6,0x98,0x75),(0xe6,0x7e,0x80),(0xd6,0x99,0xb6),(0x7f,0xbb,0xb3),(0x23,0x2a,0x2e)]
def lerp(a, b, t): return int(a + (b - a) * t)
def color(t):
    t = max(0.0, min(1.0, t)); seg = t * (len(stops) - 1); i = int(seg); f = seg - i
    if i >= len(stops) - 1: return stops[-1]
    a, b = stops[i], stops[i + 1]
    return (lerp(a[0],b[0],f), lerp(a[1],b[1],f), lerp(a[2],b[2],f))
raw = bytearray()
for y in range(H):
    raw.append(0)
    for x in range(W):
        t = x/(W-1)*0.62 + y/(H-1)*0.38
        r, g, b = color(t)
        raw += bytes((r, g, b))
def chunk(typ, data):
    c = typ + data
    return struct.pack(">I", len(data)) + c + struct.pack(">I", zlib.crc32(c) & 0xffffffff)
png = b"\x89PNG\r\n\x1a\n"
png += chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
png += chunk(b"IEND", b"")
open(path, "wb").write(png)
PY

echo "fixture ready at $FIXTURE"
git --no-pager log --oneline
