<div align="center">

# Oyo

A diff viewer for stepping through changes and reviewing scrollable diffs.

<!-- Regenerate with: cd website && npm run generate-demos -->
![Oyo demo](docs/assets/demo.gif)

</div>

Oyo is a terminal diff viewer.

Use it as a normal scrollable diff viewer, or step through changes one at a time. You can switch between both modes at any time.

## Choose a review mode

### Scroll mode

Use scroll mode when you want a traditional diff viewer.

You can:

- review all changes at once
- scroll freely
- jump between hunks

Start with:

```sh
oy
```

### Step-through diff

Use step-through mode when you want to see how a file changes over time. This helps with large refactors and careful reviews.

Start with:

```sh
oy --step
```

You can also press `s` in the TUI, or set `stepping = true` in config.

## Features

- scroll mode
- step-through navigation
- hunk navigation
- unified, split, evolution, blame and preview views
- tabs for multiple views
- mouse support
- Markdown, JSON, YAML, TOML, CSV and image previews
- inline review comments
- visual text selection with copy, comments and command actions
- word-level diffing
- multi-file navigation
- watch mode that refreshes changed files on disk
- regex search
- syntax highlighting
- blame hints
- command palette
- line wrap
- context folding
- animated transitions
- autoplay
- Git integration
- History view with `oy log`
- built-in themes and `.tmTheme` syntax themes
- XDG config file support

## Install

### npm

```sh
npm i -g @ahkohd/oyo
```

### Pi package

This gives you the `/diff` and `/review` commands.

```sh
pi install npm:@ahkohd/pi-oyo
```

### Homebrew

```sh
brew install ahkohd/oyo/oy
```

### Arch Linux

```sh
paru -S oyo
```

### Cargo

```sh
cargo install oyo --locked --force
```

## Use Oyo

### Show uncommitted changes

```sh
oy
```

### Step through uncommitted changes

```sh
oy --step
```

### Compare 2 files

```sh
oy old.rs new.rs
```

### Compare one file with HEAD

```sh
oy path/to/file.rs
```

### Show staged changes

```sh
oy --staged
```

### Show a Git range

```sh
oy --range HEAD~1..HEAD
oy --range main...feature
```

### Choose a view mode

```sh
oy old.rs new.rs --view split
oy old.rs new.rs --step --view evolution
```

### Use autoplay

```sh
oy old.rs new.rs --step --autoplay
oy old.rs new.rs --step --speed 100
```

### Open History

```sh
oy log
```

## Use with Git

Use `git difftool`:

```sh
git difftool -y --tool=oy
```

Add this to `~/.gitconfig`:

```gitconfig
[difftool "oy"]
    cmd = oy "$LOCAL" "$REMOTE"

[difftool]
    prompt = false

[alias]
    d = difftool -y --tool=oy
```

Keep your pager, such as `less`, `moar` or `moor`, for `git diff`.

Do not set `core.pager` or `interactive.diffFilter` to `oy`.

## Use with Jujutsu

Register `oy` as a diff tool and add a shortcut for it:

```toml
[merge-tools.oy]
program = "oy"
diff-args = ["$left", "$right"]

[aliases]
d = ["diff", "--tool", "oy"]
```

Then run `jj d` or `jj diff --tool oy`.

You can also open jj targets directly:

```sh
oy @
oy feature
oy 'trunk()..@'
```

Keep your pager for `jj diff`, `jj log` and everything else. Do not set `oy` as the global `ui.diff-formatter`.

## Review comments

Press `m` while reviewing to add a comment.

```sh
oy
```

Oyo saves review state for the current diff target.

Show review status:

```sh
oy review status
oy review status --json
```

Show comments:

```sh
oy review comment
oy review comment feature
```

In Git, `status` and `comment` default to the working tree, like `oy`. Pass a branch, commit or range for branch review comments. Use an explicit base for stacked branches, for example `oy review status development...feature`. jj defaults to `@`, unless `@` has one bookmark. In that case, Oyo treats the bookmark like a branch.

Use a jj revset to review a stack:

```sh
oy 'trunk()..@'
oy review comment 'trunk()..@'
```

Push or pull GitHub pull request comments with `gh`:

```sh
oy review pull
oy review push
```

Export or apply comments as JSON:

```sh
oy review export --format json > comments.json
oy review comment apply comments.json
```

For the full review workflow, see [review commands](./docs/REVIEW.md).

Review hooks are documented in [review hooks](./docs/REVIEW_HOOKS.md).

## Keybindings

Most navigation commands support counts. For example, `10j` moves 10 steps forward and `5J` scrolls down 5 lines.

Common defaults:

| Key | Action |
| --- | --- |
| `j`, `down` | Scroll down, or next step in step mode |
| `k`, `up` | Scroll up, or previous step in step mode |
| `l`, `right` | Next hunk |
| `h`, `left` | Previous hunk |
| `v` | Start character selection |
| `V` | Start line selection |
| `ctrl-v` | Start block selection |
| `y` | Copy selection or current line |
| `/` | Search |
| `n` | Next search match |
| `N` | Previous search match |
| `tab` | Cycle view mode |
| `s` | Toggle stepping |
| `m` | Add or update a line comment |
| `M` | Add or update a hunk comment |
| `ctrl-s` | Save an inline comment |
| `ctrl-p` | Open the command palette |
| `R` | Refresh files |
| `?` | Show help |
| `q`, `esc` | Quit |

Full keybinding and mouse reference: [keybindings](./docs/KEYBINDINGS.md).

Selection works with mouse drag or `v`, `V` and `ctrl-v`. Press `y` to copy and `esc` to clear.

Clipboard support first uses system tools:

- `pbcopy` on macOS
- `wl-copy`, `xclip` or `xsel` on Linux
- `clip` on Windows

If those fail, Oyo falls back to OSC 52 terminal clipboard support.

## Configure Oyo

Create a config file at `~/.config/oyo/config.toml`.

You can also pass extra config files with repeatable `--config FILE`. Oyo loads your user config first, then merges each extra config file in order.

```sh
oy --config /tmp/oyo-plugin.toml
```

Minimal config:

```toml
[ui]
view_mode = "unified"
stepping = false
watch = true
line_wrap = false
fold_context = "off"

[ui.diff]
fg = "syntax"
bg = true
highlight = "word"

[ui.syntax]
mode = "on"

[files]
panel_visible = true
panel_width = 30
panel_position = "left"
```

Config is loaded from the first matching file:

1. `$XDG_CONFIG_HOME/oyo/config.toml`
2. `~/.config/oyo/config.toml`
3. the platform config directory, such as `~/Library/Application Support/oyo/config.toml` on macOS

Use these docs for full configuration:

- [config reference](./docs/CONFIG.md)
- [theme configuration](./docs/THEME.md)
- [keybinding actions](./docs/KEYBINDINGS.md)
- [review commands](./docs/REVIEW.md)
- [review hooks](./docs/REVIEW_HOOKS.md)
- [diff behaviour](./docs/DIFF_VIEWER.md)
- [diff styling previews](./docs/DIFF_PREVIEWS.md)

[Diff styling previews](./docs/DIFF_PREVIEWS.md) include screenshots.

## Development

```sh
cargo build
cargo test
cargo run --bin oy -- old.rs new.rs
```
