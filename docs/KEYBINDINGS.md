# Keybindings Reference

Oyo keybindings are configured in `config.toml` with `[keybindings.<mode>]` tables.
Action names are snake_case. Omitted actions keep defaults. An empty array unbinds an action.

Example:

```toml
[keybindings.global]
open_command_palette = ["ctrl-p"]
open_file_search = ["ctrl-shift-p"]

[keybindings.normal]
step_down = ["j", "down"]
goto_start = ["g g", "home"]
open_editor = ["o", "ctrl-e"]
```

Notes:

- Key sequences use spaces: `g g`, `g b`, `ctrl-x`.
- Modifiers use hyphens: `ctrl-p`, `ctrl-shift-p`, `alt-x`, `cmd-p`.
- Common named keys: `esc`, `enter`, `tab`, `backtab`, `space`, `up`, `down`, `left`, `right`, `home`, `end`, `pagedown`, `pageup`, `backspace`, `delete`.
- Duplicate bindings or prefix conflicts make that whole mode fall back to defaults with a warning.
- In `normal`, plain `1` through `9` are reserved for counts. Plain `0` means `line_start` unless a count is already pending. Modified digits such as `ctrl-1` are allowed.
- `global` is checked before text input modes, except `help` and `review_editor`.
- `normal.open_command_palette` and `normal.open_file_search` still work in normal mode, but use `global` if the shortcuts should work while a picker, search box, or filter is active.

## Modes

| Mode | When used |
| --- | --- |
| `global` | Global app shortcuts before most input modes |
| `normal` | Main diff view |
| `help` | Help popover |
| `review_editor` | Inline comment editor |
| `command_palette` | Command palette picker |
| `file_search` | Quick file search picker |
| `file_filter` | File panel filter |
| `goto` | Goto prompt |
| `search` | Diff search prompt |
| `dashboard` | Commit picker dashboard |
| `dashboard_filter` | Dashboard filter prompt |

## `global`

| Action | Default keys | Description |
| --- | --- | --- |
| `open_command_palette` | `ctrl-p` | Command palette |
| `open_file_search` | `ctrl-shift-p` | Quick file search |

## `normal`

| Action | Default keys | Description |
| --- | --- | --- |
| `quit` | `q`, `esc` | Quit and print comments if any |
| `step_down` | `j`, `down` | Step forward |
| `step_up` | `k`, `up` | Step backward |
| `next_hunk` | `l`, `right` | Next hunk |
| `prev_hunk` | `h`, `left` | Previous hunk |
| `hunk_start` | `b` | Hunk begin |
| `hunk_end` | `e` | Hunk end |
| `blame_hint` | `g b` | Blame current step |
| `toggle_peek_change` | `p` | Peek change |
| `toggle_peek_hunk` | `P` | Peek old hunk |
| `yank_change` | `y` | Yank line |
| `yank_hunk` | `Y` | Yank hunk |
| `yank_change_patch` | `g y` | Copy line patch |
| `yank_hunk_patch` | `g Y` | Copy hunk patch |
| `toggle_path_popup` | `ctrl-g` | Show full file path |
| `open_editor` | `o`, `ctrl-e` | Open file in editor |
| `goto_start` | `g g`, `home` | Go to start |
| `goto_end` | `G`, `end` | Go to end |
| `first_step` | `<` | First step, or hunk in no-step |
| `last_step` | `>` | Last step, or hunk in no-step |
| `prev_file` | `[` | Previous file |
| `next_file` | `]` | Next file |
| `toggle_autoplay` | `space` | Autoplay forward |
| `toggle_autoplay_reverse` | `B` | Autoplay reverse |
| `toggle_view_mode` | `tab` | Cycle view mode |
| `toggle_view_mode_reverse` | `backtab` | Cycle view mode reverse |
| `scroll_up` | `K` | Scroll up |
| `scroll_down` | `J` | Scroll down |
| `half_page_up` | `ctrl-u` | Scroll half-page up |
| `half_page_down` | `ctrl-d` | Scroll half-page down |
| `toggle_file_list_focus` | `enter`, `ctrl-a` | Focus file list |
| `increase_speed` | `+`, `=` | Increase speed |
| `decrease_speed` | `-` | Decrease speed |
| `toggle_animation` | `a` | Toggle animation |
| `toggle_line_wrap` | `w` | Toggle line wrap |
| `toggle_syntax` | `t` | Toggle syntax highlight |
| `toggle_evo_syntax` | `E` | Toggle evo syntax |
| `toggle_stepping` | `s` | Toggle stepping |
| `toggle_strikethrough` | `S` | Toggle strikethrough |
| `scroll_left` | `H` | Scroll left |
| `scroll_right` | `L` | Scroll right |
| `line_start` | `0` | Scroll to line start |
| `line_end` | `$` | Scroll to line end |
| `center_active` | `z` | Center on active |
| `toggle_zen` | `Z` | Zen mode |
| `replay_step` | `r` | Replay last step |
| `refresh` | `R` | Refresh files |
| `toggle_file_panel` | `ctrl-f` | Toggle file panel |
| `toggle_fold_context` | `f` | Toggle context folding |
| `open_search_or_file_filter` | `/` | Search or filter files |
| `open_goto` | `:` | Go to line, hunk, or step |
| `search_next` | `n` | Next match |
| `search_prev` | `N` | Previous match |
| `next_conflict` | `c` | Next conflict |
| `prev_conflict` | `C` | Previous conflict |
| `line_comment` | `m` | Add or update line comment |
| `hunk_comment` | `M` | Add or update hunk comment |
| `clear_comments` | `ctrl-x` | Clear all comments |
| `remove_line_comment` | `x` | Remove line comment |
| `remove_hunk_comment` | `X` | Remove hunk comment |
| `toggle_help` | `?` | Toggle help |
| `open_command_palette` | `ctrl-p` | Command palette in normal mode |
| `open_file_search` | `ctrl-shift-p` | Quick file search in normal mode |

## `help`

| Action | Default keys | Description |
| --- | --- | --- |
| `close` | `esc`, `q`, `?` | Close help |
| `scroll_down` | `j`, `down` | Scroll down |
| `scroll_up` | `k`, `up` | Scroll up |

## `review_editor`

| Action | Default keys | Description |
| --- | --- | --- |
| `cancel` | `esc` | Cancel editor |
| `save` | `ctrl-enter` | Save comment |
| `insert_newline` | `enter` | Insert newline |
| `accept_mention` | `tab` | Accept mention |
| `backspace` | `backspace` | Backspace |
| `delete` | `delete` | Delete |
| `left` | `left` | Move left |
| `right` | `right` | Move right |
| `up` | `up` | Move up |
| `down` | `down` | Move down |
| `home` | `home` | Move to line start |
| `end` | `end` | Move to line end |
| `clear` | `ctrl-u` | Clear text |
| `mention_next` | `ctrl-n` | Next mention candidate |
| `mention_prev` | `ctrl-p` | Previous mention candidate |

## `command_palette`

| Action | Default keys | Description |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Accept |
| `backspace` | `backspace` | Backspace |
| `clear` | `ctrl-u` | Clear query |
| `select_next` | `down` | Select next |
| `select_prev` | `up` | Select previous |

## `file_search`

| Action | Default keys | Description |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Accept |
| `backspace` | `backspace` | Backspace |
| `clear` | `ctrl-u` | Clear query |
| `select_next` | `down` | Select next |
| `select_prev` | `up` | Select previous |

## `file_filter`

| Action | Default keys | Description |
| --- | --- | --- |
| `close` | `esc`, `enter` | Close filter |
| `backspace` | `backspace` | Backspace |
| `clear` | `ctrl-u` | Clear filter |

## `goto`

| Action | Default keys | Description |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Accept |
| `backspace` | `backspace` | Backspace |
| `clear` | `ctrl-u` | Clear query |

## `search`

| Action | Default keys | Description |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Accept |
| `backspace` | `backspace` | Backspace |
| `clear` | `ctrl-u` | Clear query |

## `dashboard`

| Action | Default keys | Description |
| --- | --- | --- |
| `quit` | `esc`, `q` | Quit dashboard |
| `start_filter` | `/` | Filter commits |
| `clear_pin` | `r` | Clear pinned range start |
| `toggle_pin` | `space` | Mark range start |
| `accept` | `enter` | Open selection |
| `select_next` | `j`, `down` | Select next |
| `select_prev` | `k`, `up` | Select previous |
| `page_down` | `pagedown` | Page down |
| `page_up` | `pageup` | Page up |
| `select_first` | `g`, `home` | Select first |
| `select_last` | `G`, `end` | Select last |

## `dashboard_filter`

| Action | Default keys | Description |
| --- | --- | --- |
| `cancel` | `esc` | Cancel filter |
| `accept` | `enter` | Open selection |
| `clear` | `ctrl-u` | Clear filter |
| `backspace` | `backspace` | Backspace |
| `select_next` | `j`, `down` | Select next |
| `select_prev` | `k`, `up` | Select previous |
| `page_down` | `pagedown` | Page down |
| `page_up` | `pageup` | Page up |
| `select_first` | `g`, `home` | Select first |
| `select_last` | `G`, `end` | Select last |
