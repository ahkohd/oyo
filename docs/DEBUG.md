# Debug view logger

Use the debug view logger to inspect diff rendering without opening the TUI.

It writes the render state to a log file. Use it when you need to check per-line decisions, including hunk extent markers.

## Turn on logging

Set `OYO_DEBUG_VIEW=1` before you run `oy`:

```sh
OYO_DEBUG_VIEW=1 OYO_DEBUG_VIEW_CLEAR=1 oy
```

By default, Oyo writes logs to `/tmp/oyo_view_debug.log`.

## Set optional variables

| Variable | What it does |
| --- | --- |
| `OYO_DEBUG_VIEW_FILE=/path/to/log` | Writes the log to this file |
| `OYO_DEBUG_VIEW_CLEAR=1` | Clears the log on the first write |
| `OYO_DEBUG_VIEW_CONTEXT=2` | Includes 2 context lines above and below the visible render window |
| `OYO_DEBUG_VIEW_MAX_LINES=400` | Limits log lines per snapshot. Use `0` for no limit |
| `OYO_DEBUG_VIEW_EVERY=1` | Logs every render, even when state has not changed |
| `OYO_DEBUG_VIEW_FILTER=pattern[,pattern...]` | Logs only files whose path contains a pattern. Matching is case-insensitive |
| `OYO_DEBUG_VIEW_STEP=step|nostep|any` | Logs only step mode, no-step mode, or both. Default is `any` |
| `OYO_DEBUG_VIEW_NAV=1` | Adds user navigation events to the log |

## Read a snapshot

Each snapshot starts with a header:

```text
OYO_VIEW_DEBUG ts_ms=... pane=unified file_index=0 file="path/to/file"
mode=UnifiedPane stepping=false line_wrap=false diff_status=Ready placeholder=false view_len=1234 windowed=true window_start=0 window_total=5000 viewport_h=40 viewport_w=120 scroll_global=200 render_scroll=200
state current_hunk=3 total_hunks=8 last_nav_was_hunk=true cursor_change=512 show_extent_step=false scope_hunk=3 scope_from_cursor=true step_direction=None animation_phase=Idle
visible_render_range=200..239 context=2
```

Per-line entries follow:

```text
L raw=210 disp=200-200 gdisp=200-200 h=3 scope=true show=true kind=Context changes=false old=100 new=100 act=false prim=false id=512 wrap=1 txt="..."
```

Use these fields first:

| Field | Meaning |
| --- | --- |
| `disp` | Display index range in the current render window |
| `gdisp` | Global display index range, with the window offset applied |
| `h` | Hunk index, or `-` when there is none |
| `scope` | Whether the line is inside the current hunk scope |
| `show` | Whether `ViewLine.show_hunk_extent` is set |
| `kind` | Line type, such as context, added or deleted |
| `changes` | Whether the line contains actual changes |
| `old` and `new` | Old and new line numbers, when present |

Split mode logs old and new display indices instead of one `disp` value.

## Debug extent markers

Use this logger when extent markers do not cover the full hunk.

Capture a log around the bug, then check:

- `scope=true` lines where `show=false`
- lines at `visible_render_range` boundaries
- changes in `scope_hunk`, `last_nav_was_hunk` and `cursor_change`

Viewport edges are often where missing markers show up.

## Log navigation

Set `OYO_DEBUG_VIEW_NAV=1` with `OYO_DEBUG_VIEW=1` to log navigation actions.

A navigation entry looks like this:

```text
OYO_VIEW_NAV ts_ms=... action=step_down moved=true file_index=0 file="path/to/file" view_mode=UnifiedPane stepping=true scroll_global=200 render_scroll=40 window_start=160 windowed=true current_step=12 current_hunk=3 cursor_change=512 last_nav_was_hunk=true step_direction=Forward
```

`action` is one of:

- `step_down`
- `step_up`
- `hunk_down`
- `hunk_up`

`moved` tells you whether the action changed the view or cursor state.
