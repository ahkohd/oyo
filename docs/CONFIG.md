# Config reference

Use this page to find Oyo config keys and defaults.

Create your config file at `~/.config/oyo/config.toml`.

You can also pass extra config files. Oyo loads your user config first, then merges each extra file in order:

```sh
oy --config /tmp/oyo-plugin.toml
```

Extra config files append `review.hooks`, `review.actions` and `selection.actions`. They merge tables, such as `keybindings.normal`.

## Config file locations

Oyo loads the first config file it finds:

1. `$XDG_CONFIG_HOME/oyo/config.toml`.
2. `~/.config/oyo/config.toml`.
3. The platform config directory, such as `~/Library/Application Support/oyo/config.toml` on macOS.

## Minimal config

```toml
[ui]
view_mode = "unified"
stepping = false
watch = true
line_wrap = false

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

## UI

Use `[ui]` for general display and navigation defaults.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `zen` | `true`, `false` | `false` | Starts with minimal UI chrome |
| `topbar` | `true`, `false` | `true` | Shows the top bar in the diff view |
| `auto_center` | `true`, `false` | `true` | Keeps the active change centred while stepping |
| `watch` | `true`, `false` | `true` | Refreshes changed files on disk |
| `overscroll` | `true`, `false` | `false` | Allows extra end-of-file scroll while centring |
| `confirm_quit` | `true`, `false` | `true` | Asks for confirmation before quitting the TUI |
| `view_mode` | `unified`, `split`, `evolution`, `blame`, `preview` | `unified` | Sets the default view mode |
| `line_wrap` | `true`, `false` | `false` | Wraps long lines instead of horizontal scrolling |
| `fold_context` | `off`, `expandable` | `expandable` | Sets whether Oyo folds long unchanged blocks |
| `fold_context_lines` | non-negative integer | `3` | Keeps this many context lines on each side of a fold |
| `scrollbar` | `true`, `false` | `true` | Shows the diff and file panel scrollbars |
| `strikethrough_deletions` | `true`, `false` | `false` | Strikes through deleted text |
| `gutter_signs` | `true`, `false` | `true` | Shows added and removed signs in unified and evolution views |
| `stepping` | `true`, `false` | `false` | Starts in step mode |
| `primary_marker` | string | built in | Sets the active-line marker for the left pane or unified view |
| `primary_marker_right` | string | built in | Sets the active-line marker for the right pane |
| `extent_marker_left` | string | built in | Sets the hunk extent marker for the left pane or unified view |
| `extent_marker_right` | string | built in | Sets the hunk extent marker for the right pane |
| `extent_marker_deleted` | string | built in | Sets the hunk extent marker for deleted lines |
| `extent_marker` | string | built in | Legacy name for `extent_marker_left` |

Long unchanged blocks are expandable by default. Oyo creates a fold when at least 8 lines remain hidden after preserving edge context. Set `fold_context = "off"` to show full context by default. Set `fold_context_lines = 0` for the most compact expandable view. The older `on` value remains an alias for `expandable`.

Press `f` to toggle full and folded context. Search checks rendered lines. Press `F` to expand all folds before searching hidden context.

```toml
[ui]
zen = false
topbar = true
auto_center = true
watch = true
overscroll = false
confirm_quit = true
view_mode = "unified"
line_wrap = false
fold_context = "expandable"
fold_context_lines = 3
scrollbar = true
strikethrough_deletions = false
gutter_signs = true
stepping = false
extent_marker_left = "┃"
extent_marker_right = "▐"
extent_marker_deleted = "╏"

# Use "off" to show full context by default.
# Use fold_context_lines = 0 for maximum compaction.

[ui.toasts]
enabled = true
position = "bottom_right"
```

### Toasts

Use `[ui.toasts]` to turn toast notifications on or off and choose where they appear.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `enabled` | `true`, `false` | `true` | Shows short notifications for actions such as copy and toggles |
| `position` | `top_left`, `top_right`, `bottom_left`, `bottom_right`, `center` | `bottom_right` | Sets where toast notifications appear |

## Diff display

Use `[ui.diff]` to control diff colours, inline highlights and large-file behaviour.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `bg` | `true`, `false` | `true` | Shows full-line diff backgrounds |
| `fg` | `theme`, `syntax` | `syntax` | Chooses diff text colours |
| `highlight` | `text`, `word`, `none` | `word` | Controls inline changed-span highlights |
| `max_bytes` | integer | `16777216` | Defers diffing above this file size |
| `full_context_max_bytes` | integer | `2097152` | Uses full-context rendering up to this file size |
| `defer` | `true`, `false` | `true` | Computes large diffs in the background |
| `idle_ms` | integer | `250` | Waits this many milliseconds before background diffing starts |
| `extent_marker` | `neutral`, `diff` | `diff` | Chooses hunk extent marker colour |
| `extent_marker_scope` | `progress`, `hunk` | `hunk` | Chooses which lines get diff colours while stepping |
| `extent_marker_context` | `true`, `false` | `false` | Shows extent markers on unchanged context lines |
| `preview_change_bars` | `true`, `false` | `true` | Shows change bars in source, Markdown, CSV and structured previews |

```toml
[ui.diff]
bg = true
fg = "syntax"
highlight = "word"
max_bytes = 16777216
full_context_max_bytes = 2097152
defer = true
idle_ms = 250
extent_marker = "diff"
extent_marker_scope = "hunk"
extent_marker_context = false
preview_change_bars = true
```

See [diff behaviour](./DIFF_VIEWER.md) and [diff styling previews](./DIFF_PREVIEWS.md).

## Syntax highlighting

Use `[ui.syntax]` to turn syntax highlighting on or off and choose a syntax theme.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `mode` | `on`, `off` | `on` | Enables syntax highlighting |
| `theme` | theme name or path | empty | Uses `ui.theme.name`, then falls back to `ansi` |

Use `[ui.syntax.warmup]` to control background syntax preparation.

| Key | Default | What it does |
| --- | --- | --- |
| `active_lines` | `100` | Lines per tick while navigating |
| `pending_lines` | `300` | Lines per tick while catching up |
| `idle_lines` | `1000` | Lines per tick while idle |
| `debounce_ms` | `80` | Wait time before warming a new viewport target |

```toml
[ui.syntax]
mode = "on"
theme = "tokyonight"

[ui.syntax.warmup]
active_lines = 100
pending_lines = 300
idle_lines = 1000
debounce_ms = 80
```

See [theme configuration](./THEME.md).

## View-specific settings

Use these tables to change one view mode.

```toml
[ui.unified]
modified_step_mode = "mixed" # "mixed" or "modified"

[ui.split]
align_lines = true
align_fill = "/"

[ui.evo]
syntax = "context" # "context" or "full"
```

| Table | Key | Values | Default | What it does |
| --- | --- | --- | --- | --- |
| `[ui.unified]` | `modified_step_mode` | `mixed`, `modified` | `mixed` | Chooses how modified lines render while stepping |
| `[ui.split]` | `align_lines` | `true`, `false` | `true` | Inserts blank rows to keep panes aligned |
| `[ui.split]` | `align_fill` | string | built in | Fills aligned blank rows |
| `[ui.evo]` | `syntax` | `context`, `full` | `context` | Chooses syntax scope in evolution view |

## Blame and time

Use `[ui.blame]` for git blame hints.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `enabled` | `true`, `false` | `false` | Shows blame hints |
| `mode` | `one_shot`, `toggle` | `one_shot` | Chooses how blame display behaves |
| `hunk_hint` | `true`, `false` | `true` | Shows blame hint when jumping to a hunk |

Use `[ui.time]` for time display.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `mode` | `relative`, `absolute`, `custom` | `relative` | Chooses time display mode |
| `format` | string | empty | Sets the custom time format |

```toml
[ui.blame]
enabled = false
mode = "one_shot"
hunk_hint = true

[ui.time]
mode = "relative"
format = "[year]-[month]-[day] [hour]:[minute]"
```

## Theme

Use `[ui.theme]` to choose a UI theme.

```toml
[ui.theme]
name = "tokyonight"
mode = "dark"
```

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `name` | theme name | unset | Sets the UI theme |
| `mode` | `dark`, `light` | dark | Chooses the theme variant |
| `defs` | table | empty | Defines reusable colour names |
| `theme` | table | built in | Overrides theme tokens |

See [theme configuration](./THEME.md) for built-in themes, custom themes and theme tokens.

## Navigation wrapping

Use `[navigation.wrap]` to choose what happens at the ends of files or hunks.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `step` | `none`, `step`, `file` | `none` | Chooses step navigation wrapping |
| `hunk` | `none`, `hunk`, `file` | `none` | Chooses hunk navigation wrapping |

```toml
[navigation.wrap]
step = "none"
hunk = "none"
```

## Playback

Use `[playback]` for autoplay, animations and automatic first-step behaviour.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `speed` | integer | `200` | Autoplay delay in milliseconds |
| `autoplay` | `true`, `false` | `false` | Starts with autoplay enabled |
| `animation` | `true`, `false` | `true` | Enables step animations |
| `animation_duration` | integer | `120` | Animation duration in milliseconds |
| `auto_step_on_enter` | `true`, `false` | `true` | Steps to the first change when entering a file at step 0 |
| `auto_step_blank_files` | `true`, `false` | `true` | Steps when a new file would otherwise be blank at step 0 |

```toml
[playback]
speed = 200
autoplay = false
animation = true
animation_duration = 120
auto_step_on_enter = true
auto_step_blank_files = true
```

## Files

Use `[files]` to configure the file panel.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `panel_visible` | `true`, `false` | `true` | Shows the file panel in multi-file mode |
| `panel_width` | integer | `30` | Sets file panel width in columns |
| `panel_position` | `left`, `right` | `left` | Sets which side the file panel uses |
| `counts` | `active`, `focused`, `all`, `off` | `active` | Chooses when per-file counts appear |

Use `[files.scan]` for manual directory scans.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `git_ignore` | `auto`, `true`, `false`, `on`, `off` | `auto` | Uses git ignore rules during scans |
| `ignore_globs` | array of strings | VCS directories | Excludes paths during scans |

```toml
[files]
panel_visible = true
panel_width = 30
panel_position = "left"
counts = "active"

[files.scan]
git_ignore = "auto"
ignore_globs = [".git/**", ".jj/**", ".hg/**", ".svn/**"]
```

## No-step mode

Use `[no_step]` for scroll mode behaviour.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `auto_jump_on_enter` | `true`, `false` | `true` | Jumps to the first hunk when entering a file in no-step mode |

```toml
[no_step]
auto_jump_on_enter = true
```

## Editor

Use `[editor]` for the open-in-editor action.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `command` | string | `$VISUAL`, then `$EDITOR`, then `vi` | Sets the editor command |
| `args` | array of strings | unset | Sets argument templates. Supports `{file}` and `{line}` |
| `open_at_line` | `true`, `false` | `true` | Passes `+line` before the file path when `args` is unset |

```toml
[editor]
command = "nvim"
args = ["+{line}", "{file}"]
open_at_line = true
```

## Comments and mentions

Use `[comments.mentions]` to configure file mentions in comments.

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `file_scope` | `changed`, `repo` | `repo` | Chooses files available for mentions |
| `finder` | `auto`, `builtin`, `fzf` | `auto` | Chooses how Oyo ranks mention candidates |

```toml
[comments.mentions]
file_scope = "repo"
finder = "auto"
```

## Review database and sync

Use `[review]` to set where Oyo saves the review database.

```toml
[review]
dir = ".oyo/reviews"
```

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `dir` | path | platform app data directory | Sets the base directory for review databases |

Relative paths resolve from the current workspace root. This supports Git worktrees and jj workspaces.

GitHub sync uses the built-in `gh` adapter. Use provider config when you need to define another provider command interface.

Pass a remote name to `oy review pull` or `oy review push` when you do not want the default remote.

### Provider command interface

Add custom providers under `[review.providers.<id>]`.

```toml
[review.providers.example]
hosts = ["git.example.com"]

[review.providers.example.commands.whoami]
command = "example-oyo-provider"
args = ["whoami"]

[review.providers.example.commands.pr_get]
command = "example-oyo-provider"
args = ["pr", "get", "{repo}", "{target}"]

[review.providers.example.commands.comments_list]
command = "example-oyo-provider"
args = ["comments", "list", "{repo}", "{number}"]

[review.providers.example.commands.comments_create]
command = "example-oyo-provider"
args = ["comments", "create", "{repo}", "{number}"]

[review.providers.example.commands.comments_update]
command = "example-oyo-provider"
args = ["comments", "update", "{repo}", "{number}"]

[review.providers.example.commands.comments_delete]
command = "example-oyo-provider"
args = ["comments", "delete", "{repo}", "{number}"]
```

Provider command keys are:

- `whoami`
- `pr_find`
- `pr_get`
- `comments_list`
- `comments_create`
- `comments_update`
- `comments_delete`

Each command supports:

| Key | Values | Default | What it does |
| --- | --- | --- | --- |
| `command` | string | none | Sets the executable |
| `args` | list of strings | `[]` | Sets command arguments |
| `timeout_ms` | number | `30000` | Sets the command timeout |

Provider commands return Oyo-shaped JSON. Mutation commands read Oyo-shaped JSON from standard input.

See [provider command contract](./REVIEW.md#provider-command-contract).

## Review hooks and actions

Use `[[review.hooks]]` to run commands after review events.

```toml
[[review.hooks]]
id = "review-ready"
on = "review_ready"
command = ".oyo/hooks/review-ready"
args = []
stdin = "json"
blocking = true
timeout_ms = 30000
```

Use `[[review.actions]]` to expose commands in the UI.

```toml
[[review.actions]]
id = "send"
label = "Send review"
key = "ctrl-s"
on = "review_ready"
command = ".oyo/hooks/send-review"
args = []
stdin = "json"
blocking = true
timeout_ms = 30000
save_editor = true
show = ["command_palette", "review_editor"]
```

Review events are:

- `comment_saved`
- `comment_deleted`
- `comments_cleared`
- `review_ready`

`stdin` can be `json` or `none`.

See [review hooks](./REVIEW_HOOKS.md).

## Selection actions

Use `[[selection.actions]]` to run commands from the selection toolbar.

```toml
[[selection.actions]]
id = "ask-agent"
label = "Ask agent"
key = "a"
message = "Sent to agent"
failure_message = "Could not send to agent"
command = "my-agent"
args = ["review-selection"]
stdin = "json"
blocking = true
timeout_ms = 30000
```

Oyo sends JSON on standard input by default. The payload includes the selected text, file, repo root, line ranges, view mode, side, scroll offset and selected screen rows.

`stdin` can be `json` or `none`.

If you omit `message` or `failure_message`, Oyo uses `<label> started` and `<label> failed`.

## Keybindings

Use `[keybindings.<mode>]` tables to override keys.

```toml
[keybindings.global]
open_command_palette = ["ctrl-p"]
open_file_search = ["ctrl-shift-p"]
open_theme_picker = ["ctrl-t"]

[keybindings.normal]
step_down = ["j", "down"]
step_up = ["k", "up"]
open_dashboard = ["ctrl-r"]
start_selection = ["v"]
start_line_selection = ["V"]
start_block_selection = ["ctrl-v"]
toggle_help = ["?"]

[keybindings.review_editor]
save = ["ctrl-s"]
clear = ["ctrl-u"]

[keybindings.selection]
copy = ["y"]
cancel = ["esc"]
show_actions = ["enter"]

[keybindings.search]
cancel = ["esc"]
accept = ["enter"]
clear = ["ctrl-u"]
```

If you omit an action, Oyo keeps the default keys. Set an action to an empty array to unbind it.

See [keybinding actions](./KEYBINDINGS.md).
