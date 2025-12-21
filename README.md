# oyo

Step through changes one at a time and watch the code transform, unlike traditional diff tools that show a static before/after.

## Features

- **Step-through navigation**: Move through changes one at a time with keyboard shortcuts
- **Hunk navigation**: Jump between groups of related changes (hunks) with `h` and `l`
- **Animated transitions**: Smooth fade in/out animations as changes are applied
- **Three view modes**:
  - **Single pane**: Watch the code morph from old to new state
  - **Side-by-side**: See old and new versions with synchronized stepping
  - **Evolution**: Watch the file evolve, deletions simply disappear
- **Git integration**: Works as a git external diff tool or standalone
- **Word-level diffing**: See exactly which words changed within a line
- **Autoplay**: Automatically step through all changes at a configurable speed
- **Multi-file support**: Navigate between changed files with preserved positions
- **Configurable**: XDG config file support for customization

## Installation

### CLI (Rust)

```bash
# From source
git clone https://github.com/ahkohd/oyo
cd oyo
cargo install --path crates/oyo
```

## Usage

### CLI

```bash
# Diff uncommitted changes in current git repo
oyo

# Compare two files
oyo old.rs new.rs

# Side-by-side view
oyo old.rs new.rs --view side-by-side

# Evolution view
oyo old.rs new.rs --view evolution

# Autoplay mode
oyo old.rs new.rs --autoplay

# Custom autoplay speed (100ms between steps)
oyo old.rs new.rs --speed 100
```

### Git Integration

`oyo` works seamlessly with git:

```bash
# Use as git external diff (one-off)
git -c diff.external=oyo diff

# Configure permanently
git config --global diff.external oyo

# Then just use git normally
git diff
git show --ext-diff
git log -p --ext-diff
```

Recommended git aliases in `~/.gitconfig`:

```gitconfig
[alias]
    # Step-through diff aliases
    dlog = -c diff.external=oyo log --ext-diff
    dshow = -c diff.external=oyo show --ext-diff
    ddiff = -c diff.external=oyo diff
```

### Keyboard Shortcuts

**Vim-style counts**: Most navigation commands support count prefixes (e.g., `10j` moves 10 steps forward, `5J` scrolls down 5 lines).

| Key | Action |
|-----|--------|
| `↓` / `j` | Next step (supports count) |
| `↑` / `k` | Previous step (supports count) |
| `→` / `l` | Next hunk (supports count) |
| `←` / `h` | Previous hunk (supports count) |
| `<` | First step |
| `>` | Last step |
| `g` | Go to start (scroll + first step) |
| `G` | Go to end (scroll + last step) |
| `Space` | Toggle autoplay |
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
| `r` | Refresh file (or all files when file list focused) |
| `f` | Toggle file panel |
| `]` | Next file (supports count) |
| `[` | Previous file (supports count) |
| `+` / `=` | Increase speed |
| `-` | Decrease speed |
| `?` | Toggle help |
| `q` / `Esc` | Quit (or close help) |

## Configuration

Create a config file at `~/.config/oyo/config.toml`:

```toml
[ui]
auto_center = true          # Auto-center on active change (default: true)
view_mode = "single"        # Default: "single", "side-by-side", or "evolution"
line_wrap = false           # Wrap long lines (default: false, uses horizontal scroll)
scrollbar = false           # Show scrollbar (default: false)
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
auto_step_on_enter = false  # Auto-step to first change when entering a file

[files]
panel_visible = true        # Show file panel in multi-file mode
```

Config is loaded from (in priority order):
1. `$XDG_CONFIG_HOME/oyo/config.toml`
2. `~/.config/oyo/config.toml`
3. Platform-specific (e.g., `~/Library/Application Support/oyo/config.toml` on macOS)

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
cargo run --bin oyo -- old.js new.js
```

## License

MIT
