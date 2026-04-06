# pi-oyo

Pi package that adds `oy`-backed `/diff` and `/review` commands.

## Install

From npm:

```bash
pi install npm:@ahkohd/pi-oyo
```

From a local checkout:

```bash
pi install ./packages/pi-oyo
```

## Requirements

- `oy` available in `PATH`

## Commands

- `/diff [oy args...]`
  - Opens `oy` and returns to Pi
- `/review [oy args...]`
  - Opens `oy`, captures comments on quit, and pastes them into the editor
- `/review temp [oy args...]`
  - Same as `/review`, but runs an ephemeral session (`--no-review-persist`)
- `/review new [oy args...]`
  - Same as `/review`, but clears saved session state first (`--clear-review-session`)
