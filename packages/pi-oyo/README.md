# pi-oyo

Pi package that points the agent at the installed Oyo skills.

When enabled, it adds short routing instructions to the agent's system prompt:

- code reviews and comments use `oy skill path`
- guided walkthroughs use `oy skill path walkthrough`
- live TUI control uses `oy skill path control`

The agent reads the matching skill and drives `oy` itself.

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

- `/oyo [on|off|status]`
  - `on` (default): add the Oyo skill instructions to the agent's system prompt
  - `off`: stop adding them
  - `status`: report the current mode

The mode persists across a session. The default is `on`.
