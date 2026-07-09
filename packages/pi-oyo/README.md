# pi-oyo

Pi package that points the agent at the Oyo code-review skill.

When enabled, it appends a short instruction to the agent's system prompt telling it to
read the installed Oyo skill (`oy skill path`) and use `oy` / `oy review` for reviews and
Oyo comments. That is all — the agent drives `oy` itself.

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
  - `on` (default): inject the Oyo skill instruction into the agent's system prompt
  - `off`: stop injecting it
  - `status`: report the current mode

The mode persists across a session; the default is `on`.
