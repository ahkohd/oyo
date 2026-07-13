---
name: oyo-tui-control
description: Use oy control to steer a running Oyo TUI through the CLI. Inspect sessions, navigate files and hunks, switch targets and modes, and guide live code reviews without faking key presses.
---

# Oyo TUI control

The TUI belongs to the user. Steer it with `oy control`; do not fake key presses or start another interactive Oyo.

If no session exists, ask the user to start Oyo first:

```sh
oy
oy --session review-a
```

Use the short alias when it is clearer:

```sh
oy c where --json
```

## Workflow

```text
1. oy control list --json                         # find live sessions
2. oy control where --session review-a --json     # inspect the current focus
3. oy control diff --session review-a --json      # inspect files and hunks
4. oy control target --session review-a TARGET    # change target if needed
5. oy control goto --session review-a ...         # move to the right place
6. oy review comment ...                          # create or update review data
7. oy control where --session review-a --json     # check lastAppliedSeq after queued work
8. oy control cancel --session review-a           # stop queued work if needed
```

## Session selection

Most commands accept `--session <name>` or `-s <name>`.

Use one of these forms:

```sh
oy control list
oy control where
oy control where --session review-a
oy control where -s review-a
oy control where --session 42811
```

- if one session is running for the current workspace, the session flag is optional
- if more than one session matches, run `oy control list` and pass `--session` or `-s`
- use `oy --session review-a` when you start Oyo if you want a stable name

## Commands

### Inspect

```sh
oy control list [--json]
oy control get --session review-a [--json]
oy control where --session review-a [--json]
oy control diff --session review-a --json
oy control diff --session review-a --json --include-patch
```

- `list` shows running sessions
- `get` shows session metadata, including workspace and target
- `where --json` shows the current file, cursor, selection, active tab and `lastAppliedSeq`
- `diff --json` shows files and hunks without raw patch text by default
- add `--include-patch` only when you need patch text

### Navigate

```sh
oy control goto --session review-a --file src/lib.rs --new-line 42
oy control goto --session review-a --file src/lib.rs --old-line 39
oy control goto --session review-a --file src/lib.rs --hunk 2
oy control goto --session review-a --step-number 12
oy control goto --session review-a --start
oy control goto --session review-a --end

oy control next --session review-a --count 3
oy control prev --session review-a
oy control hunk --session review-a next
oy control hunk --session review-a prev
oy control hunk --session review-a start
oy control hunk --session review-a end
oy control file --session review-a src/lib.rs
oy control file --session review-a src/lib.rs --new-tab
oy control file --session review-a next
oy control file --session review-a prev
```

- use exactly one navigation target with `goto`
- line and hunk numbers are 1-based
- `next` and `prev` use the current step mode
- use the full file path when a suffix matches more than one file

### Target

```sh
oy control target --session review-a @
oy control target --session review-a feature/review-sync
oy control target --session review-a development...feature
oy control target --session review-a --worktree
oy control target --session review-a --staged
```

- `target` changes what the running TUI shows
- use `--worktree` for the Git working tree
- use `--staged` for the Git index
- pass the same Git or jj target you would pass to `oy`
- if the command is queued, poll `oy control where --json` until `lastAppliedSeq` reaches the returned `seq`

### Modes

```sh
oy control view --session review-a unified
oy control view --session review-a split
oy control view --session review-a evolution
oy control view --session review-a blame
oy control view --session review-a preview
oy control view --session review-a next
oy control view --session review-a prev

oy control step --session review-a on
oy control step --session review-a off
oy control step --session review-a toggle
oy control watch --session review-a on
oy control watch --session review-a off
oy control watch --session review-a toggle

oy control speed --session review-a increase
oy control speed --session review-a decrease
oy control animation --session review-a toggle
oy control wrap --session review-a toggle
oy control syntax --session review-a toggle
oy control zen --session review-a toggle

oy control sidebar --session review-a files
oy control sidebar --session review-a comments
oy control sidebar --session review-a close
oy control sidebar --session review-a toggle
oy control sidebar --session review-a focus

oy control tab --session review-a file src/lib.rs
oy control tab --session review-a help
oy control tab --session review-a settings
oy control tab --session review-a pr-comments
oy control tab --session review-a outdated-comments
oy control tab --session review-a close

oy control play --session review-a --from current --to end --delay 700ms
oy control pause --session review-a
oy control cancel --session review-a
oy control action --session review-a normal.step_down --count 3
oy control action --session review-a normal.navigate_back
oy control action --session review-a normal.navigate_forward
oy control rename --session review-a review-b
```

- use `on`, `off` or `toggle` for boolean modes
- use `pause` to stop autoplay
- use `cancel` to clear queued control work and stop motion
- prefer named commands over `action` when they exist
- use `action` for supported TUI actions that do not need their own command

### Comments

Use `oy review` for review data. Do not mutate comments with `oy control`.

```sh
oy review status --unresolved
oy review comment --unresolved
oy review comment --since 1783478786 --json

oy review comment new --file src/lib.rs --new-line 42 --body "Handle empty input."
oy review comment edit 1 --body "Handle empty input before parsing."
oy review comment resolve 1
oy review comment unresolve 1
oy review comment rm 1 --yes
```

- `oy control` steers the view
- `oy review` creates, edits, resolves and deletes comments
- the running TUI reloads review database changes
- set `OYO_REVIEW_AUTHOR_TYPE=agent` when comments should show an agent author

## Special cases

### Queued commands

Some commands return before the TUI finishes applying them. This includes play, target reloads and animated movement.

If a response includes `queued: true` and `seq`, wait like this:

```sh
oy control where --session review-a --json
```

Check `lastAppliedSeq`. Continue when it is greater than or equal to the queued `seq`.

Use this to stop queued work:

```sh
oy control cancel --session review-a
```

### User input wins

If the user types or clicks, Oyo stops queued agent work. Re-check the current state with:

```sh
oy control where --session review-a --json
```

### Raw patch text

Do not request raw patch text by default. Start with:

```sh
oy control diff --session review-a --json
```

Add `--include-patch` only for the files or checks that need patch text.

### Multiple sessions

Give long-running reviews a name:

```sh
oy --session review-a
```

Rename a running session when needed:

```sh
oy control rename --session oyo-42811 review-a
```

## Guiding a review

Use control commands to show the user what matters. Use review commands to leave durable notes.

Typical flow:

1. Run `oy control list --json` and select the session.
2. Run `oy control where --json` to see what the user is viewing.
3. Run `oy control diff --json` to scan files and hunks.
4. Navigate to the first useful file or hunk.
5. Explain what the user is looking at.
6. Add comments with `oy review comment new` when the note should persist.
7. Move through the rest of the review in the clearest order.
8. Summarise what you checked and what remains.

Guidelines:

- navigate before commenting so the user sees the code you mean
- tell a clear story, not necessarily file order
- comment on risks, intent and follow-up work
- do not comment on every hunk
- use `oy review comment --unresolved` as the work queue when fixing comments

## Common errors

- `No Oyo sessions running.` - start Oyo, then run `oy control list` again
- `No Oyo session is running for this workspace.` - start Oyo in this workspace, or pass `--session`
- `More than one Oyo session is running. Pass --session.` - run `oy control list`, then pass the session name
- "Failed to connect to Oyo session 'NAME'. Run `oy control list`." - run `oy control list` to remove stale sessions, then retry
- `No visible diff file matches PATH.` - check `oy control diff --json`, then use a visible path or change target
- `More than one visible diff file matches PATH.` - pass the full path
- `Specify exactly one navigation target` - use one of `--new-line`, `--old-line`, `--hunk`, `--step-number`, `--start` or `--end`
- `Pass a target, --worktree or --staged` - add a target to `oy control target`
- `Control queue full. Cancel or wait.` - run `oy control cancel` or wait for `lastAppliedSeq`
- `Use on, off or toggle` - pass one of those values to the mode command
- `Unknown view mode: MODE` - use `unified`, `split`, `evolution`, `blame`, `preview`, `next` or `prev`
- `Unsupported control action: ID` - use a supported action ID, or use a named control command
- `Delay must be a positive duration` - use a value such as `700ms` or `1s`
- `Pass a file path` - pass a file path to `oy control file` or `oy control tab file`
