# Record the hero demo

The website hero shows a real `oy` session. We record it as an [asciicast v2 file](https://docs.asciinema.org/manual/asciicast/v2/) and play it with [asciinema-player](https://docs.asciinema.org/manual/player/).

The recording is scripted, so you can re-record it when the Oyo interface changes.

The demo tools are in `website/scripts/demo/`. The recorded file is `website/public/demo.cast`.

## How the recording works

The script runs `oy` inside asciinema, and runs asciinema inside pilotty:

```sh
pilotty spawn -- asciinema rec --window-size 112x34 -f asciicast-v2 -c "oy ..." demo.cast
```

`pilotty` owns the outer pseudo-terminal and sends input to it. It sends named keys as real terminal input, so keys such as `ctrl-r`, `ctrl-s`, `esc`, arrows and mouse clicks reach `oy`.

`pilotty wait-for` and `pilotty snapshot` let the script wait for text on screen. This is more reliable than fixed sleeps.

`asciinema` records the output from `oy` with real timing. The script forces `asciicast-v2` so the website player can read the file.

## Why the script uses pilotty

Do not use `tmux send-keys` or raw bytes for this demo. They lose input the demo needs.

Raw byte input can drop or rewrite:

- arrow key escape sequences
- mouse reports
- control key combinations such as `ctrl-s` and `ctrl-o`

`pilotty` sends real key events, so these inputs work.

## What you need

Install these tools before you record:

- `pilotty`: `npm install -g pilotty`
- `asciinema`: `cargo install asciinema` or `brew install asciinema`
- `node`: used by the helper scripts
- `oy`: build it with `cargo build --release`

## Re-record the demo

Run these commands from the repository root:

```sh
cargo build --release
website/scripts/demo/setup-repo.sh
website/scripts/demo/record.sh
```

`setup-repo.sh` creates a fixture repository in `/tmp`.

`record.sh` drives `oy` and writes `website/public/demo.cast`.

You can override these settings:

- `OYO_BIN`: path to the `oy` binary
- `OYO_DEMO_THEME`: theme name, default `evergarden-winter`
- `OYO_DEMO_FIXTURE`: path to the fixture repository

The recording size is `112x34`.

## What the demo shows

The fixture is a small repository created by `website/scripts/demo/setup-repo.sh`. It has 4 commits and working tree changes.

The changed files include:

- a Rust file
- Markdown
- JSON
- YAML
- CSV
- an untracked PNG image

The recording script shows these Oyo features in order:

1. It opens a multi-file diff with the sidebar.
2. It shows unified word-level diff and hunk navigation.
3. It adds an inline review comment with `m`, text input and `ctrl-s`.
4. It starts step-through playback with `s` and `space`.
5. It switches to split view and blame view.
6. It opens previews for image, CSV, YAML, JSON and Markdown files.
7. It opens a second tab and switches tabs by clicking the tab bar.
8. It opens History with `ctrl-r`.

## Check the recording without a display

Recording is headless. Use `vt-snap.mjs` to reconstruct a frame from the cast:

```sh
node website/scripts/demo/vt-snap.mjs website/public/demo.cast 12
node website/scripts/demo/vt-snap.mjs website/public/demo.cast
```

The first command shows the screen at 12 seconds. The second command shows the final frame.

`vt-snap.mjs` is a small VT emulator. It handles cursor movement, erase sequences and newlines. It strips colour, so use it to check layout and content only.

Check colours and image output in a browser.

During script authoring, you can inspect the live terminal with:

```sh
pilotty snapshot --format text -s oyodemo
```

## Common issues

Pilotty mouse rows start at 0. To click a visible screen row shown as row N in a 1-based count, use `click N-1 COL`.

End the demo by quitting `oy`. Do not kill pilotty. If you kill pilotty, asciinema may not flush the file. `record.sh` sends `q` until `oy` exits, then waits for asciinema to print `Recorded`.

`trim.mjs` removes the trailing terminal teardown from the cast. This stops the looping player from flashing the restored shell.

To pause on the current frame, wait for text that will not appear:

```sh
pilotty wait-for "text that never appears" -t 1500
```

This waits for about 1.5 seconds, then times out.

`--idle-time-limit 2` caps long idle gaps, such as `oy` startup, so the cast does not contain dead time.

Oyo renders images as unicode half-blocks. asciinema-player can replay these. Do not use a terminal graphics protocol such as kitty or sixel for this demo.

`--no-review-persist` stops review comments from leaking between runs.

## Embed the recording on the website

`website/src/pages/index.astro` loads `website/public/demo.cast` with the self-hosted asciinema-player standalone bundle.

The player files are vendored into `public/vendor/`. The standalone bundle inlines its web worker as a Blob, so it needs no worker path rewrite. It works under the website base path and content security policy.

The player uses:

- autoplay
- loop
- no controls
- a static frame when the user prefers reduced motion
- a static frame when JavaScript is unavailable

## Files

- `website/scripts/demo/setup-repo.sh`: creates the fixture repository
- `website/scripts/demo/record.sh`: drives `oy` and writes the cast
- `website/scripts/demo/demo.config.toml`: sets the narrow sidebar for the recording
- `website/scripts/demo/vt-snap.mjs`: reconstructs a screen from a cast at a given time
- `website/scripts/demo/trim.mjs`: removes trailing terminal teardown from a cast
