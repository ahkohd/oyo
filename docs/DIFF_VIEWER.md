# Diff viewer behaviour

Use this page to understand how Oyo shows diffs, stepping, hunk navigation and styling.

This page describes user-visible behaviour. It does not describe implementation details.

## Terms

| Term | Meaning |
| --- | --- |
| Change | One diff change, such as an insertion, deletion or modified line |
| Step | One move through the diff changes |
| Active change | The current step target, shown as the cursor line |
| Applied change | A change you have stepped into, so the new state is visible |
| Hunk | A group of nearby changes |
| Hunk preview | A temporary full-hunk view after hunk navigation |

## Navigate and step through changes

The key examples below use the default bindings. You can change them in `[keybindings.<mode>]` in `config.toml`. See [keybindings](./KEYBINDINGS.md).

Use `j` and `k` to step forward and backward through changes.

Use `h`, `l` or `:h<num>` to jump between hunks.

When you enter a hunk by jumping:

- Oyo shows a full preview of that hunk
- the cursor lands at the top when you jump forward
- the cursor lands at the bottom when you jump backward
- extent markers stay visible while you are inside the hunk
- extent markers clear when you leave the hunk

After a hunk preview, the first step returns to normal stepping:

- step forward keeps the first change, applies the next change and continues top to bottom
- step backward removes the last applied change and continues bottom to top

Use `p` to cycle the modified line view:

- modified
- old
- mixed

Use `P` to peek at the old state for the current hunk.

## Unified view

Unified view shows one stream. The file changes as you step.

Modified lines behave like this:

| State | What you see |
| --- | --- |
| Before step | Old text |
| On step | Mixed old and new inline text |
| After step | New text |

Insertions behave like this:

| State | What you see |
| --- | --- |
| Before step | Hidden |
| On step | New text, active |
| After step | New text |

Deletions behave like this:

| State | What you see |
| --- | --- |
| Before step | Old text |
| On step | Old text, active. It fades out when animation is enabled |
| After step | Hidden |

A hunk preview shows the full hunk with all changes applied. The first step after the preview returns to progressive stepping.

## Split view

Split view shows the old file on the left and the new file on the right.

Modified lines behave like this:

| State | Left | Right |
| --- | --- | --- |
| Before step | Old text | Old text |
| On step | Old text, active | New text, active |
| After step | Old text | New text |

Inline word-level diffs stay visible after you step through a modified line.

Insertions behave like this:

| State | What you see |
| --- | --- |
| Before step | The right pane shows old or context text |
| On step | The right pane shows new text |
| After step | The right pane shows new text |

Deletions behave like this:

| State | What you see |
| --- | --- |
| Before step | The left pane shows old text |
| On step | The left pane shows old text with deletion styling |
| After step | The left pane shows old text with deletion styling |

## Evolution view

Evolution view shows the file changing over time.

In evolution view:

- deleted lines disappear
- delete markers are hidden
- diff background is always off
- `ui.evo.syntax` controls syntax highlighting

Set `ui.evo.syntax` to:

| Value | What it does |
| --- | --- |
| `context` | Uses syntax only on non-diff lines |
| `full` | Uses syntax on diff and context lines. The active line keeps diff colours |

Use `E` to toggle this setting in evolution view.

## Preview view

Preview view shows the file content instead of the diff.

In preview view:

- Markdown files render as Markdown
- JSON, YAML and TOML files show an interactive tree
- CSV files show a table
- PNG, JPEG, GIF, WebP and BMP files show an image preview in preview view
- other text files show source text with syntax highlighting when syntax is on
- source, Markdown, CSV and structured previews show change bars when `ui.diff.preview_change_bars` is on
- deleted files preview the old side
- other files preview the new side
- the top-right toggle switches Markdown, CSV and structured previews between preview and source

In JSON, YAML and TOML preview:

- `j` and `k` move between values
- `h` collapses a value or moves to its parent
- `l` expands a value or moves to its first child
- `space` toggles the current value
- `c` and `C` collapse sibling values
- `e` and `E` expand sibling values
- `m` switches between data view and line view
- `gg` and `G` move to the start and end
- `ctrl-u` and `ctrl-d` jump half a page

In CSV preview:

- `j` and `k` move between rows
- `h` and `l` move between cells
- `gg` and `G` move to the start and end

## No-step mode

No-step mode is the config name for scroll mode.

In no-step mode:

- all changes are applied at once
- changed lines show extent markers
- `j` and `k` scroll
- `h` and `l` jump between hunks
- stepping is disabled
- hunk preview is disabled

## Context folds

Oyo folds long unchanged blocks by default. It keeps 3 lines on each side and only
adds a fold when at least 8 lines remain hidden. When syntax data identifies an
enclosing function, method, class or section, Oyo shows its definition line dimmed
on the right of the fold. Narrow views truncate the line before removing fold
controls. Markdown sections use the nearest enclosing heading. Files over 512 KiB
omit scope hints to keep folding responsive.

Use the contextual shortcut beside an arrow, such as `ua` or `da`, or click the
shortcut and arrow button to reveal 20 lines from that side. Use `F` to expand all
folds. Use `f` to toggle between folded and full context.

Fold rows are interface controls. Visual selection skips them, and copied text does
not include them. Inline review comments keep their line and nearby context open,
including resolved comments. Outdated comments do not open folded context. Search
only checks rendered lines, so expand folds before searching hidden context.

Set `ui.fold_context = "off"` to start with full context. Set
`ui.fold_context_lines = 0` for the most compact expandable view.

## Scrollbar

Use `ui.scrollbar` to show or hide scrollbars.

The scrollbar is on by default. It appears when the diff or file panel is longer than the visible area.

Set `ui.scrollbar = false` to hide it.

## Diff foreground

Use `ui.diff.fg` to choose whether diff text uses theme colours or syntax colours.

| Value | What it does |
| --- | --- |
| `theme` | Uses diff colours from the UI theme |
| `syntax` | Uses syntax colours on non-active lines. The active line stays in diff colours |

## Diff background

Use `ui.diff.bg` to turn full-line diff backgrounds on or off.

This setting applies to unified and split views. Evolution view ignores it.

| Value | What it does |
| --- | --- |
| `false` | Shows no full-line background |
| `true` | Shows full-line backgrounds, including gutter line numbers and signs |

Cursor markers and extent markers do not take the background colour.

## Inline highlights

Use `ui.diff.highlight` to control inline highlights.

This setting applies to unified and split views. Evolution view ignores it.

| Value | What it does |
| --- | --- |
| `text` | Highlights changed spans, including leading whitespace |
| `word` | Highlights changed spans, excluding leading whitespace |
| `none` | Turns inline highlights off |

## Large diffs

Use `ui.diff.max_bytes` to defer diffing for large files.

Files larger than this value are shown immediately, then diffed in the background.

Use `ui.diff.full_context_max_bytes` to choose when Oyo switches from full-context rendering to limited context rendering.

If a file is deferred, Oyo first renders it in scroll mode. It upgrades to a full diff when background computation finishes.

Use `ui.diff.defer = true` to enable deferred diffing.

Use `ui.diff.idle_ms` to set how long Oyo waits after the last input before background diffing starts.

## Extent markers

Extent markers show the current hunk or step area.

In no-step mode, changed lines show extent markers all the time.

Use `ui.diff.extent_marker` to choose marker colour:

| Value | What it does |
| --- | --- |
| `neutral` | Uses the neutral marker colour |
| `diff` | Uses the line's diff colour |

Use `ui.diff.extent_marker_scope` to choose how much of the hunk gets diff colours:

| Value | What it does |
| --- | --- |
| `progress` | Uses diff colours only on already-applied change lines |
| `hunk` | Uses diff colours on all lines in the current hunk |

Set `ui.diff.extent_marker_context = true` to show extent markers on unchanged context lines.

## Line wrap

Line wrap is visual only. Navigation still uses logical lines.

When auto-centre is on, Oyo uses wrapped display metrics to keep the active line visible.

## Config example

```toml
[ui]
auto_center = true      # keep active change centred while stepping
overscroll = false      # allow EOF overscroll when centring
scrollbar = true        # show scrollbars

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

[ui.evo]
syntax = "context"
```
