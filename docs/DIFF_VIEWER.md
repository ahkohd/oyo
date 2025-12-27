# Diff Viewer Spec

This document defines user-visible behavior for stepping, hunk navigation, view
mode rendering, and syntax/diff styling. It is intentionally behavior-focused
and avoids implementation details.

## Terminology

- **Change**: A single diff change (insert/delete/modify) at a line.
- **Step**: One unit of progression through changes.
- **Active change**: The current step target (cursor line).
- **Applied change**: A change that has been stepped into (visible as new state).
- **Hunk**: A group of nearby changes.
- **Hunk preview**: Temporary full-hunk view after hunk navigation.

## Navigation & Stepping

- **Step forward/back** (`j` / `k`): apply/unapply the next/previous change.
- **Hunk jump** (`h` / `l`, `:h<num>`): jump to previous/next hunk.
  - Entering a hunk via hunk jump shows a **full preview** of that hunk.
  - Cursor lands at the **top** of the hunk when jumping forward, and at the
    **bottom** when jumping backward.
  - Extent markers remain visible while inside the hunk and clear when leaving it.
- **First step after hunk preview** collapses the preview into normal stepping:
  - **Step forward**: keeps the first change, applies the next, and proceeds
    top-to-bottom.
  - **Step backward**: removes the last applied change and proceeds
    bottom-to-top.
- **Peek change** (`p`): cycles single-line modified view: `modified -> old -> mixed`.
- **Peek old hunk** (`P`): temporarily shows old state for the current hunk.

## View Modes

### Single

Single view shows a single stream that morphs as you step.

**Modified line lifecycle (single view):**

| State | What you see |
| --- | --- |
| Before step | Old text |
| On step | Mixed (old + new inline) |
| After step | New text |

**Insertions (single view):**
- Before step: hidden
- On step: new text (active)
- After step: new text

**Deletions (single view):**
- Before step: old text
- On step: old text (active, fades out if animation enabled)
- After step: hidden

Hunk preview shows the full hunk (all changes applied); first step collapses to
progressive stepping.

### Split

Split view shows old on the left, new on the right.

**Modified line lifecycle (split view):**

| State | Left | Right |
| --- | --- | --- |
| Before step | Old text | Old text |
| On step | Old text (active) | New text (active) |
| After step | Old text | New text |

Inline word-level diffs remain visible after stepping through a modified line.

**Insertions (split view):**
- Before step: right shows old/context
- On/after step: right shows new text

**Deletions (split view):**
- Before step: left shows old text
- On/after step: left shows old text (with delete styling)

### Evolution

- Deleted lines disappear (no delete markers).
- Diff background is always **off** to keep the morph view clean.
- Syntax scope is controlled by `ui.evo.syntax`:
  - `context`: syntax only on non-diff lines.
  - `full`: syntax on diff + context lines (active line stays in diff colors).
  - Toggle via `E` (Evolution view only).

## No-step Mode

- All changes are applied at once (scroll-only diff viewer).
- `j`/`k` scroll; `h`/`l` jump between hunks.
- Stepping and hunk preview are disabled.

## Styling Rules

### Foreground (diff vs syntax)

- `ui.diff.fg = "theme"`: diff colors drive text.
- `ui.diff.fg = "syntax"`: syntax colors for non-active lines; active line stays
  in diff colors to retain focus.

### Background (diff.bg)

Applies to single/split only (ignored in evolution):

- `false`: no full-line background.
- `true`: full-line background including gutter (line numbers/signs), but
  cursor/ext markers do not take background.

### Inline highlight (diff.highlight)

Applies to single/split only (ignored in evolution):

- `text`: highlight changed spans including leading whitespace.
- `word`: like `text`, but leading whitespace is not highlighted.
- `none`: disable inline highlights.

### Extent markers

- `ui.diff.extent_marker = "neutral"`: hunk extent markers use the neutral marker color.
- `ui.diff.extent_marker = "diff"`: hunk extent markers take the line’s diff color.
- `ui.diff.extent_marker_scope = "progress"`: only already-applied change lines use diff colors.
- `ui.diff.extent_marker_scope = "hunk"`: all lines in the current hunk use diff colors.

## Line Wrap

- Wrap is visual-only; navigation still operates on logical lines.
- Auto-center uses wrapped display metrics to keep the active line visible.

## Config Snapshot

```toml
[ui]
[ui.diff]
bg = false            # true | false
fg = "theme"          # theme | syntax
highlight = "text"    # text | word | none
extent_marker = "neutral" # neutral | diff
extent_marker_scope = "progress" # progress | hunk

[ui.evo]
syntax = "context"    # context | full
```
