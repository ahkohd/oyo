<div align="center">

# oyo

A Step-through diff viewer.

<!-- Demo source: https://github.com/user-attachments/assets/dd284411-48cb-4015-b500-0246629c2493 -->
https://github.com/user-attachments/assets/dd284411-48cb-4015-b500-0246629c2493

</div>


Step through changes or scroll the full diff, jump between hunks, and watch code transform instead of a static before/after.

## Features

- **Step-through navigation**: Move through changes one at a time with keyboard shortcuts
- **No-step mode**: Review all changes at once with scroll + hunk navigation (scroll-only diff viewer)
- **Hunk navigation**: Jump between groups of related changes (hunks) in step or no-step mode
- **Animated transitions**: Smooth fade in/out animations as changes are applied
- **Syntax highlighting**: Toggle on/off for code-aware coloring (auto-enabled in no-step mode)
- **Three view modes**:
  - **Single**: Watch the code morph from old to new state
  - **Split**: See old and new versions with synchronized stepping
  - **Evolution**: Watch the file evolve, deletions simply disappear
- **Git integration**: Works as a git external diff tool or standalone
- **Word-level diffing**: See exactly which words changed within a line
- **Autoplay**: Automatically step through all changes at a configurable speed
- **Multi-file support**: Navigate between changed files with preserved positions
- **Configurable**: XDG config file support for customization

## Installation

### CLI (Cargo)

```bash
cargo install oyo
```

## Usage

Optional theme override:

```bash
oyo --theme-name tokyonight
```

### CLI

```bash
# Diff uncommitted changes in current git repo
oyo

# Compare two files
oyo old.rs new.rs

# Split view
oyo old.rs new.rs --view split

# Evolution view
oyo old.rs new.rs --view evolution

# Autoplay mode
oyo old.rs new.rs --autoplay

# Custom autoplay speed (100ms between steps)
oyo old.rs new.rs --speed 100

# No-step mode
oyo old.rs new.rs --no-step

# Staged changes (index vs HEAD)
oyo --staged

# Git range
oyo --range HEAD~1..HEAD
# or
oyo --range main...feature
```

### Git Integration

`oyo` supports git external diff (7-arg interface) and `git difftool`. For most workflows,
`difftool` is smoother; `diff.external` will open a TUI per file.

```bash
# One-off (recommended)
git difftool -y --tool=oyo
```

Recommended `~/.gitconfig`:

```gitconfig
[difftool "oyo"]
    cmd = oyo "$LOCAL" "$REMOTE"

[difftool]
    prompt = false

[alias]
    d = difftool -y --tool=oyo
    oyo = "!oyo"
```

External diff setup (optional):

```bash
git config --global diff.external oyo
```

Note: keep your pager (e.g., `less`, `moar`, `moor`) for normal `git diff` output.
Do not set `core.pager` to `oyo`. Also avoid `interactive.diffFilter` — it expects
a stdin filter, not a TUI.

### Jujutsu (jj)

In `~/.config/jj/config.toml`:

```toml
[ui]
paginate = "never"
diff-formatter = ["oyo", "$left", "$right"]
```

To use `jj diff --tool=oyo`:

```toml
[diff-tools.oyo]
command = ["oyo", "$left", "$right"]
```

Note: do not set your `ui.pager` to `oyo`.

Example range diff:

```bash
jj diff -f zy -t w
```

### Keyboard Shortcuts

**Vim-style counts**: Most navigation commands support count prefixes (e.g., `10j` moves 10 steps forward, `5J` scrolls down 5 lines).

| Key | Action |
|-----|--------|
| `↓` / `j` | Next step (scrolls in no-step mode) |
| `↑` / `k` | Previous step (scrolls in no-step mode) |
| `→` / `l` | Next hunk (scrolls in no-step mode) |
| `←` / `h` | Previous hunk (scrolls in no-step mode) |
| `b` | Jump to beginning of current hunk (scrolls in no-step mode) |
| `e` | Jump to end of current hunk (scrolls in no-step mode) |
| `p` / `P` | Peek old (change/hunk) |
| `y` / `Y` | Yank line/hunk to clipboard |
| `/` | Search (diff pane, regex) |
| `n` / `N` | Next/previous match |
| `<` | First applied step |
| `>` | Last step |
| `gg` | Go to start (scroll-only in no-step mode) |
| `G` | Go to end (scroll-only in no-step mode) |
| `Space` / `B` | Autoplay forward/reverse |
| `Tab` | Toggle view mode |
| `K` | Scroll up (supports count) |
| `J` | Scroll down (supports count) |
| `H` | Scroll left (supports count) |
| `L` | Scroll right (supports count) |
| `0` | Start of line (horizontal) |
| `$` | End of line (horizontal) |
| `Ctrl+u` | Half page up |
| `Ctrl+d` | Half page down |
| `Ctrl+g` | Show full file path |
| `z` | Center on active change |
| `Z` | Toggle zen mode |
| `a` | Toggle animations |
| `w` | Toggle line wrap |
| `t` | Toggle syntax highlight |
| `s` | Toggle stepping (no-step mode) |
| `S` | Toggle strikethrough |
| `r` | Refresh file (or all files when file list focused) |
| `f` | Toggle file panel |
| `]` | Next file (supports count) |
| `[` | Previous file (supports count) |
| `+` / `=` | Increase speed |
| `-` | Decrease speed |
| `?` | Toggle help |
| `q` / `Esc` | Quit (or close help) |

Clipboard support uses system tools: `pbcopy` (macOS), `wl-copy` / `xclip` / `xsel` (Linux), `clip` (Windows).
Search is case-insensitive regex; invalid patterns fall back to literal matching.

## Configuration

Create a config file at `~/.config/oyo/config.toml`:

```toml
[ui]
auto_center = true          # Auto-center on active change (default: true)
view_mode = "single"        # Default: "single", "split", or "evolution"
line_wrap = false           # Wrap long lines (default: false, uses horizontal scroll)
scrollbar = false           # Show scrollbar (default: false)
strikethrough_deletions = false # Show strikethrough on deleted text
stepping = true             # Enable stepping (false = no-step mode)
syntax = "auto"             # "auto" (no-step only), "on", or "off"
# theme = { name = "tokyonight" } # Built-ins listed below
# Optional syntax tokens (fallbacks apply if omitted):
# syntaxPlain, syntaxKeyword, syntaxString, syntaxNumber, syntaxComment,
# syntaxType, syntaxFunction, syntaxVariable, syntaxConstant,
# syntaxOperator, syntaxPunctuation
# Example:
# [ui.theme.theme]
# syntaxKeyword = { dark = "darkPurple", light = "lightPurple" }
primary_marker = "▶"        # Marker for primary active line (single-width char recommended)
primary_marker_right = "◀"  # Right pane marker (optional, defaults to ◀)
extent_marker = "▌"         # Left pane extent marker (Left Half Block)
extent_marker_right = "▐"   # Right pane extent marker (optional, defaults to ▐)
zen = false                 # Start in zen mode (minimal UI)

[playback]
speed = 200                 # Autoplay interval in milliseconds
autoplay = false            # Start with autoplay enabled
animation = false           # Enable fade animations
animation_duration = 150    # Animation duration per phase (ms)
auto_step_on_enter = true   # Auto-step to first change when entering a file
auto_step_blank_files = true # Auto-step when file would be blank at step 0 (new files)
delay_modified_animation = 200 # Delay before modified lines animate to new state (ms)

[files]
panel_visible = true        # Show file panel in multi-file mode
counts = "active"           # Per-file +/- counts: active, focused, all, off
```

Config is loaded from (in priority order):
1. `$XDG_CONFIG_HOME/oyo/config.toml`
2. `~/.config/oyo/config.toml`
3. Platform-specific (e.g., `~/Library/Application Support/oyo/config.toml` on macOS)

## Themes

Pick a built-in theme:

```toml
[ui]
theme = { name = "tokyonight" }
```

Built-in themes:
`aura`, `ayu`, `catppuccin`, `catppuccin-frappe`, `catppuccin-macchiato`, `cobalt2`,
`cursor`, `dracula`, `everforest`, `flexoki`, `github`, `gruvbox`, `kanagawa`,
`lucent-orng`, `material`, `matrix`, `mercury`, `monokai`, `nightowl`, `nord`,
`one-dark`, `opencode`, `orng`, `palenight`, `rosepine`, `solarized`, `synthwave84`,
`tokyonight`, `vercel`, `vesper`, `zenburn`.

Customize or create a theme by defining color tokens in config:

```toml
[ui.theme.defs]
accent = "#ff966c"
bg = "#1a1b26"

[ui.theme.theme]
background = { dark = "bg" }
accent = { dark = "accent" }
syntaxKeyword = { dark = "#c099ff" }
```

Supported tokens:
`text`, `textMuted`, `primary`, `secondary`, `accent`, `error`, `warning`, `success`, `info`,
`background`, `backgroundPanel`, `backgroundElement`, `border`, `borderActive`, `borderSubtle`,
`diffAdded`, `diffRemoved`, `diffContext`, `diffLineNumber`, `diffExtMarker`,
`syntaxPlain`, `syntaxKeyword`, `syntaxString`, `syntaxNumber`, `syntaxComment`, `syntaxAttribute`,
`syntaxType`, `syntaxFunction`, `syntaxVariable`, `syntaxConstant`, `syntaxBuiltin`,
`syntaxMacro`, `syntaxOperator`, `syntaxPunctuation`.

You can also use `crates/oyo/themes/schema.json` as a reference when creating a theme file.

## How It Works

1. **Diff Computation**: The diff engine compares old and new content, producing a list of changes (insertions, deletions, modifications)

1. **Change Ordering**: Changes are ordered sequentially as they appear in the file

1. **Step Navigation**: The navigator tracks which changes have been "applied" at each step

1. **View Rendering**: At each step, the view shows:
   - Applied changes (fully styled)
   - Active change (highlighted with animation)
   - Pending changes (dimmed or hidden)

## Inspiration

Traditional diff tools show a static "before and after" view. Oyo was inspired by the idea of **watching edits happen** - like a time-lapse of the editing process. This is especially useful for:

- **Code review**: Follow the logical progression of changes
- **Learning**: Understand how experienced developers modify code
- **Debugging**: See exactly when and where a change was introduced

## Development

```bash
# Build everything
cargo build

# Run tests
cargo test

# Run CLI in development
cargo run --bin oyo -- old.rs new.rs
```
