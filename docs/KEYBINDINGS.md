# Keybindings

Configure keybindings in `config.toml` with `[keybindings.<mode>]` tables.

Each action below is listed as `<mode>.<action>`. Action names use `snake_case`.

If you do not set an action, Oyo uses the default keys. Set an action to an empty array to unbind it.

## Example config

```toml
[keybindings.global]
open_command_palette = ["ctrl-p"]
open_file_search = ["ctrl-shift-p"]
open_review_grep = ["ctrl-shift-f"]
open_comment_picker = ["ctrl-shift-c"]
open_theme_picker = ["ctrl-t"]

[keybindings.normal]
step_down = ["j", "down"]
goto_start = ["g g", "home"]
open_editor = ["o", "ctrl-e"]
start_selection = ["v"]
start_line_selection = ["V"]
start_block_selection = ["ctrl-v"]

[keybindings.review_editor]
save = ["ctrl-s"]

[keybindings.selection]
copy = ["y"]
cancel = ["esc"]
show_actions = ["enter"]
left = ["h", "left"]
right = ["l", "right"]
up = ["k", "up"]
down = ["j", "down"]
reanchor_left = ["H"]
reanchor_right = ["L"]
reanchor_up = ["K"]
reanchor_down = ["J"]
reanchor_start = ["ctrl-g"]
reanchor_end = ["ctrl-shift-g"]
reanchor_half_page_down = ["ctrl-d"]
goto_start = ["g"]
goto_end = ["G"]
goto_half_page_down = ["d"]
```

## Key syntax

Use these rules when you write keys:

- key sequences use spaces, for example `g g`, `g b`, `ctrl-x`
- modifiers use hyphens, for example `ctrl-p`, `ctrl-shift-p`, `alt-x`, `cmd-p`
- named keys include `esc`, `enter`, `tab`, `backtab`, `space`, `up`, `down`, `left`, `right`, `home`, `end`, `pagedown`, `pageup`, `backspace` and `delete`

Oyo requests enhanced keyboard reporting so supported terminals can distinguish `ctrl-shift-p` from `ctrl-p`. Other terminals may report them as the same key. Use `g f` for files or `g c` for comments when this happens.

Duplicate bindings or prefix conflicts make that whole mode fall back to defaults. Oyo prints a warning.

In `normal` mode, plain `1` to `9` are reserved for counts. Plain `0` means `line_start` unless a count is already pending. Modified digits such as `ctrl-1` are allowed.

`global` runs before most input modes. It does not run before `help` or `review_editor`.

`normal.open_command_palette`, `normal.open_file_search`, `normal.open_review_grep`, `normal.open_comment_picker` and `normal.open_theme_picker` still work in normal mode. Use `global` if you want shortcuts to work while a picker, search box or filter is active.

In review mode, `r`, `v`, `x` and `o` are contextual when the current file shows inline comments. Use indexed actions such as `ra reply`, `va resolve` on a thread root, `xa delete` and `oa overflow`. Reply cards omit the resolve action because resolved state belongs to the thread. These prefixes take priority over their normal-mode actions while review cards are visible.

## Mouse interactions

Mouse actions use built-in behaviour and cannot be changed with keybindings.

| Action | What it does |
| --- | --- |
| Drag in the diff | Select text |
| Click a selection action | Copy, comment, cancel or run a configured command |
| Scroll a selection action row | Move through hidden selection actions |
| Click ` + ` on a diff line | Add or update a line comment |
| Click a comment card or `ia edit` | Edit that comment |
| Click `ra reply` on an inline review comment | Reply in that local or provider thread |
| Click `xa delete` on a comment card | Delete that comment |
| Click `ra reply` on a pull request conversation comment | Quote it in a new comment |
| Click a comment editor action | Save, cancel, mention or run a configured command |
| Scroll a comment editor action row | Move through hidden comment actions |
| Click the sidebar toggle | Show or hide the sidebar |
| Drag the sidebar edge | Resize the sidebar |
| Scroll the sidebar | Scroll files or comments |
| Click the sidebar filter | Filter files or comments |
| Click the filter clear button | Clear the filter |
| Click a file in the sidebar | Open it in the active tab |
| Control-click a file in the sidebar | Open it in a new tab |
| Right-click a file in the sidebar | Open the file context menu |
| Click a sidebar file context menu action | Open, open in a new tab or copy the path |
| Click a comment in the sidebar | Open it for editing |
| Click the comments sidebar overflow menu | Pull or push review comments |
| Click a comment picker item | Jump to that review comment |
| Click a Find in files scope | Search changes or all reviewed file content |
| Click a Find in files result | Open that match |
| Click an item in History | Open it |
| Control-click an item in History | Mark the range start |
| Right-click an item in History | Open the range context menu |
| Click a History context menu action | Open, mark start or mark end |
| Click an action in the History footer | Open, mark the range, clear the range or quit |
| Click the sidebar header mode label | Switch between files and comments |
| Click the footer mode label | Cycle view modes |
| Control-click the footer mode label | Cycle view modes backwards |
| Right-click the footer mode label | Pick a view mode from a context menu |
| Click the empty-state `ctrl-r history` action | Open History |
| Click the footer file count | Open the sidebar in files mode |
| Click the footer comment count | Open the sidebar in comments mode |
| Scroll over the tab bar | Move through tabs |
| Drag a tab | Reorder tabs |
| Click a tab overflow control | Move through hidden tabs |
| Shift and scroll over the diff | Scroll horizontally |
| Drag a scrollbar | Scroll that panel |

## Modes

Use each mode as `[keybindings.<mode>]`.

| Mode | Config table | When Oyo uses it |
| --- | --- | --- |
| `global` | `[keybindings.global]` | Global app shortcuts before most input modes |
| `normal` | `[keybindings.normal]` | Main diff view |
| `help` | `[keybindings.help]` | Help popover |
| `review_editor` | `[keybindings.review_editor]` | Inline comment editor |
| `command_palette` | `[keybindings.command_palette]` | Command palette picker |
| `file_search` | `[keybindings.file_search]` | Quick file search picker |
| `review_grep` | `[keybindings.review_grep]` | Find in files picker |
| `comment_picker` | `[keybindings.comment_picker]` | Comment picker |
| `theme_picker` | `[keybindings.theme_picker]` | Theme picker |
| `file_filter` | `[keybindings.file_filter]` | File panel filter |
| `goto` | `[keybindings.goto]` | Go to prompt |
| `search` | `[keybindings.search]` | Diff search prompt |
| `selection` | `[keybindings.selection]` | Diff text selection |
| `dashboard` | `[keybindings.dashboard]` | History view |
| `dashboard_filter` | `[keybindings.dashboard_filter]` | History filter prompt |

## Global mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `open_command_palette` | `ctrl-p` | Open the command palette |
| `open_file_search` | `ctrl-shift-p` | Open quick file search |
| `open_review_grep` | `ctrl-shift-f` | Search reviewed file content |
| `open_comment_picker` | `ctrl-shift-c` | Open comment picker |
| `open_theme_picker` | `ctrl-t` | Open theme picker |

## Normal mode

`esc` closes the active picker, overlay, sub-view or find bar.

| Action | Default keys | What it does |
| --- | --- | --- |
| `quit` | `q` | Quit |
| `step_down` | `j`, `down` | Step forward |
| `step_up` | `k`, `up` | Step backward |
| `next_hunk` | `l`, `right` | Go to the next hunk |
| `prev_hunk` | `h`, `left` | Go to the previous hunk |
| `hunk_start` | `b` | Go to the start of the hunk |
| `hunk_end` | `e` | Go to the end of the hunk |
| `blame_hint` | `g b` | Show blame for the current step |
| `toggle_peek_change` | `p` | Peek at the change |
| `toggle_peek_hunk` | `P` | Peek at the old hunk |
| `yank_change` | `y` | Copy the current line or selection |
| `yank_hunk` | `Y` | Copy the current hunk |
| `yank_change_patch` | `g y` | Copy the current line patch |
| `yank_hunk_patch` | `g Y` | Copy the current hunk patch |
| `start_selection` | `v` | Start character selection |
| `start_line_selection` | `V` | Start line selection |
| `start_block_selection` | `ctrl-v` | Start block selection |
| `toggle_path_popup` | `ctrl-g` | Show the full file path |
| `open_editor` | `o`, `ctrl-e` | Open the file in your editor |
| `goto_start` | `g g`, `home` | Go to the start |
| `goto_end` | `G`, `end` | Go to the end |
| `first_step` | `<` | Go to the first step, or first hunk in no-step mode |
| `last_step` | `>` | Go to the last step, or last hunk in no-step mode |
| `prev_file` | `[` | Go to the previous file |
| `next_file` | `]` | Go to the next file |
| `toggle_autoplay` | `space` | Start or stop forward autoplay |
| `toggle_autoplay_reverse` | `B` | Start or stop reverse autoplay |
| `toggle_view_mode` | `tab` | Cycle view modes |
| `toggle_view_mode_reverse` | `backtab` | Cycle view modes in reverse |
| `open_dashboard` | `ctrl-r` | Open History |
| `navigate_back` | `alt-left` | Return to the previous file, tab or comment |
| `navigate_forward` | `alt-right` | Move forward through view history |
| `scroll_up` | `K` | Scroll up |
| `scroll_down` | `J` | Scroll down |
| `half_page_up` | `ctrl-u` | Scroll up half a page |
| `half_page_down` | `ctrl-d` | Scroll down half a page |
| `toggle_file_list_focus` | `enter`, `ctrl-a` | Focus the file list |
| `increase_speed` | `+`, `=` | Increase playback speed |
| `decrease_speed` | `-` | Decrease playback speed |
| `toggle_animation` | `a` | Turn animation on or off |
| `toggle_line_wrap` | `w` | Turn line wrap on or off |
| `toggle_syntax` | `t` | Turn syntax highlighting on or off |
| `toggle_evo_syntax` | `E` | Toggle evolution syntax mode |
| `toggle_stepping` | `s` | Turn step mode on or off |
| `toggle_strikethrough` | `S` | Turn deletion strikethrough on or off |
| `scroll_left` | `H` | Scroll left |
| `scroll_right` | `L` | Scroll right |
| `line_start` | `0` | Scroll to the start of the line |
| `line_end` | `$` | Scroll to the end of the line |
| `center_active` | `z` | Centre the active change |
| `toggle_zen` | `Z` | Turn zen mode on or off |
| `replay_step` | `r` | Replay the last step |
| `refresh` | `R` | Refresh files |
| `toggle_file_panel` | `ctrl-f` | Show or hide the file panel |
| `toggle_fold_context` | `f` | Toggle full and expandable context |
| `expand_all_folds` | `F` | Expand every context fold |
| `open_search_or_file_filter` | `/` | Search the diff, or filter files when the file list is focused |
| `open_goto` | `:` | Go to a line, hunk or step |
| `search_next` | `n` | Go to the next match |
| `search_prev` | `N` | Go to the previous match |
| `focus_next_comment` | `}` | Focus the next review comment |
| `focus_prev_comment` | `{` | Focus the previous review comment |
| `next_conflict` | `c` | Go to the next conflict |
| `prev_conflict` | `C` | Go to the previous conflict |
| `line_comment` | `m` | Comment on the hovered diff line, or the cursor line when no line is hovered |
| `hunk_comment` | `M` | Add or update a hunk comment |
| `clear_comments` | `ctrl-x` | Clear all comments |
| `remove_line_comment` | `x` | Remove a line comment |
| `remove_hunk_comment` | `X` | Remove a hunk comment |
| `toggle_help` | `?` | Show or hide help |
| `open_command_palette` | `ctrl-p` | Open the command palette in normal mode |
| `open_file_search` | `ctrl-shift-p`, `g f` | Open quick file search in normal mode |
| `open_review_grep` | `ctrl-shift-f` | Search reviewed file content |
| `open_comment_picker` | `g c` | Open comment picker in normal mode |
| `open_outdated_comments` | `g o` | Open outdated comments in normal mode |
| `open_settings` | `g s` | Open Settings in normal mode |
| `open_theme_picker` | `ctrl-t` | Open theme picker in normal mode |

Visible expandable folds show contextual shortcuts such as `ua` and `da`. Use the
shortcut beside an arrow to reveal 20 lines from that side. Use `F` to expand all
folds.

## Help mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `close` | `esc`, `q`, `?` | Close help |
| `scroll_down` | `j`, `down` | Scroll down |
| `scroll_up` | `k`, `up` | Scroll up |

## Review editor mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel the editor |
| `save` | `ctrl-s` | Save the comment |
| `insert_newline` | `enter` | Insert a new line |
| `accept_mention` | `tab` | Accept the mention |
| `backspace` | `backspace` | Delete the character before the cursor |
| `delete` | `delete` | Delete the character under the cursor |
| `left` | `left` | Move left |
| `right` | `right` | Move right |
| `up` | `up` | Move up |
| `down` | `down` | Move down |
| `home` | `home` | Move to the start of the line |
| `end` | `end` | Move to the end of the line |
| `clear` | `ctrl-u` | Clear text |
| `mention_next` | `ctrl-n` | Select the next mention candidate |
| `mention_prev` | `ctrl-p` | Select the previous mention candidate |

## Command palette mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Accept |
| `backspace` | `backspace` | Delete the previous character |
| `clear` | `ctrl-u` | Clear the query |
| `select_next` | `down` | Select the next item |
| `select_prev` | `up` | Select the previous item |

## File search mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Accept |
| `backspace` | `backspace` | Delete the previous character |
| `clear` | `ctrl-u` | Clear the query |
| `select_next` | `down` | Select the next item |
| `select_prev` | `up` | Select the previous item |

## Find in files mode

Find in files uses typo-tolerant fuzzy matching over files in the review. It shows one row for each match and keeps rows from the same file together. The default All scope searches complete current content. Changes searches added and deleted lines, plus the context shown around each change. Deleted files use their old content in both scopes. Oyo keeps your last scope for the session. It never searches files outside the review. The sidebar file filter and quick file search use the same fuzzy path matching.

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Open the selected match |
| `backspace` | `backspace` | Delete the previous character |
| `clear` | `ctrl-u` | Clear the query |
| `select_next` | `down`, `alt-n` | Select the next match |
| `select_prev` | `up`, `alt-p` | Select the previous match |
| `toggle_scope` | `tab` | Switch between Changes and All |
| `select_changes` | `alt-d` | Search changed and context lines |
| `select_everything` | `alt-e` | Search complete reviewed file content |

## Comment picker mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Open the comment |
| `backspace` | `backspace` | Delete the previous character |
| `clear` | `ctrl-u` | Clear the query |
| `select_next` | `down` | Select the next comment |
| `select_prev` | `up` | Select the previous comment |

## Theme picker mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Apply the theme |
| `backspace` | `backspace` | Delete the previous character |
| `clear` | `ctrl-u` | Clear the query |
| `select_next` | `down` | Preview the next theme |
| `select_prev` | `up` | Preview the previous theme |

## File filter mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `close` | `esc`, `enter` | Close the filter |
| `backspace` | `backspace` | Delete the previous character |
| `clear` | `ctrl-u` | Clear the filter |

## Go to mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Accept |
| `backspace` | `backspace` | Delete the previous character |
| `clear` | `ctrl-u` | Clear the query |

## Search mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel |
| `accept` | `enter` | Accept |
| `backspace` | `backspace` | Delete the previous character |
| `clear` | `ctrl-u` | Clear the query |

## Selection mode

Use `v`, `V` or `ctrl-v` in normal mode to start selection. Oyo then uses selection mode keybindings until you copy or cancel.

Selection works on visible diff cells. It does not include line numbers, gutters, align-fill characters or UI padding.

The selection toolbar appears above the selection after you finish a mouse selection. Press `enter` to show it for a keyboard selection. Select `y copy`, `esc cancel`, `m comment` in review mode, or any configured `selection.actions` command.

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel selection |
| `copy` | `y` | Copy selection |
| `show_actions` | `enter` | Show selection actions |
| `left` | `h`, `left` | Extend left |
| `right` | `l`, `right` | Extend right |
| `up` | `k`, `up` | Extend up |
| `down` | `j`, `down` | Extend down |
| `reanchor_left` | `H` | Move the anchor left |
| `reanchor_right` | `L` | Move the anchor right |
| `reanchor_up` | `K` | Move the anchor up |
| `reanchor_down` | `J` | Move the anchor down |
| `reanchor_start` | `ctrl-g` | Move the anchor to the first visible cell |
| `reanchor_end` | `ctrl-shift-g` | Move the anchor to the last visible cell |
| `reanchor_half_page_down` | `ctrl-d` | Move the anchor down half a page |
| `goto_start` | `g` | Extend to the first visible cell |
| `goto_end` | `G` | Extend to the last visible cell |
| `goto_half_page_down` | `d` | Extend down half a page |

## History mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `quit` | `esc`, `q` | Quit History |
| `start_filter` | `/` | Filter commits |
| `clear_pin` | `r` | Clear the range |
| `toggle_pin` | `space` | Mark the range start |
| `select_hovered` | `e` | Mark the range end |
| `accept` | `enter` | Open the selection |
| `select_next` | `j`, `down` | Select the next commit |
| `select_prev` | `k`, `up` | Select the previous commit |
| `page_down` | `pagedown` | Page down |
| `page_up` | `pageup` | Page up |
| `select_first` | `g`, `home` | Select the first commit |
| `select_last` | `G`, `end` | Select the last commit |

## History filter mode

| Action | Default keys | What it does |
| --- | --- | --- |
| `cancel` | `esc` | Cancel the filter |
| `accept` | `enter` | Open the selection |
| `clear` | `ctrl-u` | Clear the filter |
| `backspace` | `backspace` | Delete the previous character |
| `select_next` | `j`, `down` | Select the next commit |
| `select_prev` | `k`, `up` | Select the previous commit |
| `page_down` | `pagedown` | Page down |
| `page_up` | `pageup` | Page up |
| `select_first` | `g`, `home` | Select the first commit |
| `select_last` | `G`, `end` | Select the last commit |
hi
this
