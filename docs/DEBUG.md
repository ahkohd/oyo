# Debug View Logger

This document describes the debug view logger used to inspect diff rendering state without the TUI. It dumps per-line decisions (including extent markers) to a log file.

## Enable

Set `OYO_DEBUG_VIEW=1` before running `oy`.

Default log path: `/tmp/oyo_view_debug.log`

## Optional environment variables

- `OYO_DEBUG_VIEW_FILE=/path/to/log`
  Override output file.
- `OYO_DEBUG_VIEW_CLEAR=1`
  Truncate the log on first write.
- `OYO_DEBUG_VIEW_CONTEXT=2`
  Include N lines of context above/below the visible render window.
- `OYO_DEBUG_VIEW_MAX_LINES=400`
  Cap log lines per snapshot (0 = no cap).
- `OYO_DEBUG_VIEW_EVERY=1`
  Log every render, even when the state is unchanged.
- `OYO_DEBUG_VIEW_FILTER=pattern[,pattern...]`
  Only log when the file path contains one of the patterns (case-insensitive).
- `OYO_DEBUG_VIEW_STEP=step|nostep|any`
  Restrict logging to step mode or no-step mode. Default is `any`.
- `OYO_DEBUG_VIEW_NAV=1`
  Append user navigation events (step/hunk up/down) to the log.

## Example

```sh
OYO_DEBUG_VIEW=1 OYO_DEBUG_VIEW_CLEAR=1 oy
```

## What the log contains

Each snapshot begins with a header:

```
OYO_VIEW_DEBUG ts_ms=... pane=unified file_index=0 file="path/to/file"
mode=UnifiedPane stepping=false line_wrap=false diff_status=Ready placeholder=false view_len=1234 windowed=true window_start=0 window_total=5000 viewport_h=40 viewport_w=120 scroll_global=200 render_scroll=200
state current_hunk=3 total_hunks=8 last_nav_was_hunk=true cursor_change=512 show_extent_step=false scope_hunk=3 scope_from_cursor=true step_direction=None animation_phase=Idle
visible_render_range=200..239 context=2
```

Per-line entries follow:

```
L raw=210 disp=200-200 gdisp=200-200 h=3 scope=true show=true kind=Context changes=false old=100 new=100 act=false prim=false id=512 wrap=1 txt="..."
```

Field notes:

- `disp`: display index range in the current render window.
- `gdisp`: global display index range (window offset applied).
- `h`: hunk index (or `-` when none).
- `scope`: whether the line is inside the current hunk scope.
- `show`: `ViewLine.show_hunk_extent` (extent marker should render).
- `kind/changes`: line classification and whether it contains actual changes.
- `old/new`: old/new line numbers when present.

Split mode logs include `old`/`new` display indices instead of a single `disp`.

## Focus on extent marker bugs

For the no-step bug where extent markers do not cover the full hunk (especially
near viewport edges), capture logs around the repro and inspect:

- `scope=true` lines where `show=false` (markers should be missing)
- lines at `visible_render_range` boundaries where the hunk is partially visible
- changes in `scope_hunk`, `last_nav_was_hunk`, and `cursor_change`

## Navigation logs

When `OYO_DEBUG_VIEW_NAV=1` is set alongside `OYO_DEBUG_VIEW=1`, navigation
actions append a single-line entry:

```
OYO_VIEW_NAV ts_ms=... action=step_down moved=true file_index=0 file="path/to/file" view_mode=UnifiedPane stepping=true scroll_global=200 render_scroll=40 window_start=160 windowed=true current_step=12 current_hunk=3 cursor_change=512 last_nav_was_hunk=true step_direction=Forward
```

`action` is one of `step_down`, `step_up`, `hunk_down`, or `hunk_up`. `moved`
indicates whether the action changed the view/cursor state.
