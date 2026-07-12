# Control commands

Use `oy control` to steer a running Oyo TUI from another terminal or process.

`oy c` is a short alias for `oy control`.

## Start and select a session

Start Oyo normally:

```sh
oy
```

Give the session a stable name when you expect to control it:

```sh
oy --session review-a
```

List running sessions:

```sh
oy control list
oy control list --json
```

Most control commands accept `--session <name>` or `-s <name>`:

```sh
oy control where --session review-a
oy control where -s review-a
```

You can omit the session when one Oyo session is running for the current workspace. If several sessions match, list them and pass a name.

Show metadata for one session:

```sh
oy control get --session review-a
oy control get --session review-a --json
```

Rename a running session:

```sh
oy control rename --session review-a review-b
```

A process ID can also select a session:

```sh
oy control where --session 42811
```

## Inspect the current state

Use `where` to show the current file, cursor, selection, hunk, step, sidebar, active tab and view mode:

```sh
oy control where --session review-a
oy control where --session review-a --json
```

JSON output also includes `lastAppliedSeq`. Use this field to check whether queued work has finished.

Use `diff` to show the loaded files and hunks:

```sh
oy control diff --session review-a
oy control diff --session review-a --json
```

Patch text is excluded by default. Include it only when needed:

```sh
oy control diff --session review-a --json --include-patch
```

## Navigate files and changes

Move through the current step sequence:

```sh
oy control next --session review-a
oy control next --session review-a --count 3
oy control prev --session review-a
oy control prev --session review-a --count 2
```

Move between hunks or jump within the current hunk:

```sh
oy control hunk --session review-a next
oy control hunk --session review-a prev
oy control hunk --session review-a start
oy control hunk --session review-a end
oy control hunk --session review-a next --count 3
```

Select a file by path:

```sh
oy control file --session review-a src/lib.rs
oy control file --session review-a src/lib.rs --new-tab
```

Move through files:

```sh
oy control file --session review-a next
oy control file --session review-a prev
oy control file --session review-a next --count 3
```

Use the full path when a suffix matches more than one file.

## Jump to a location

Use `goto` with exactly one location target:

```sh
oy control goto --session review-a --file src/lib.rs --new-line 42
oy control goto --session review-a --file src/lib.rs --old-line 39
oy control goto --session review-a --file src/lib.rs --hunk 2
oy control goto --session review-a --step-number 12
oy control goto --session review-a --start
oy control goto --session review-a --end
```

Line, hunk and step numbers are 1-based. Pass `--file` with a line or hunk when the current file is not the intended target.

## Change the review target

Change what the running TUI shows:

```sh
oy control target --session review-a @
oy control target --session review-a feature/review-sync
oy control target --session review-a development...feature
```

Use Git working tree or staged changes:

```sh
oy control target --session review-a --worktree
oy control target --session review-a --staged
```

Pass the same commit, branch, bookmark, change ID, revset or range that you would pass to `oy`.

Target changes are queued. Check `lastAppliedSeq` before sending work that depends on the new target.

## Set the view

Set or cycle the view mode:

```sh
oy control view --session review-a unified
oy control view --session review-a split
oy control view --session review-a evolution
oy control view --session review-a blame
oy control view --session review-a preview
oy control view --session review-a next
oy control view --session review-a prev
```

Set step and watch modes:

```sh
oy control step --session review-a on
oy control step --session review-a off
oy control step --session review-a toggle
oy control watch --session review-a on
oy control watch --session review-a off
oy control watch --session review-a toggle
```

Set display and animation modes:

```sh
oy control speed --session review-a increase
oy control speed --session review-a decrease
oy control animation --session review-a on
oy control animation --session review-a off
oy control animation --session review-a toggle
oy control wrap --session review-a on
oy control wrap --session review-a off
oy control wrap --session review-a toggle
oy control syntax --session review-a on
oy control syntax --session review-a off
oy control syntax --session review-a toggle
oy control zen --session review-a on
oy control zen --session review-a off
oy control zen --session review-a toggle
```

Use `on`, `off` or `toggle` for boolean modes.

## Control the sidebar and tabs

Open, close or focus the sidebar:

```sh
oy control sidebar --session review-a files
oy control sidebar --session review-a comments
oy control sidebar --session review-a close
oy control sidebar --session review-a toggle
oy control sidebar --session review-a focus
```

Open or close topbar tabs:

```sh
oy control tab --session review-a file src/lib.rs
oy control tab --session review-a help
oy control tab --session review-a pr-comments
oy control tab --session review-a outdated-comments
oy control tab --session review-a close
```

The `file` tab requires a file path.

## Play and pause

Play visible steps with a delay:

```sh
oy control play --session review-a --from current --to end --delay 700ms
oy control play --session review-a --from start --to 12 --delay 1s
```

`--from` and `--to` accept `current`, `start`, `end` or a 1-based step number.

Pause autoplay:

```sh
oy control pause --session review-a
```

## Run a named action

Use `action` for supported TUI actions that do not have a dedicated control command:

```sh
oy control action --session review-a normal.step_down
oy control action --session review-a normal.step_down --count 3
```

Prefer a named control command when one exists. For example, use `next` instead of its action ID.

## Wait for queued commands

Some commands return before the TUI has finished applying them. This includes `play`, target changes and movement when stepping or animation is active.

A queued JSON response includes `queued: true` and a `seq` value. Poll the session:

```sh
oy control where --session review-a --json
```

Continue when `lastAppliedSeq` is greater than or equal to the returned `seq`.

Cancel queued work and stop motion:

```sh
oy control cancel --session review-a
```

Direct user input takes priority and clears queued control work. Inspect the session again after the user types or clicks.

## Leave review comments

`oy control` changes the live TUI view. It does not create, edit, resolve or remove review comments.

Use `oy review` for comment data:

```sh
oy review status --unresolved
oy review comment --unresolved
oy review comment new --file src/lib.rs --new-line 42 --body "Handle empty input."
oy review comment edit 1 --body "Handle empty input before parsing."
oy review comment resolve 1
oy review comment unresolve 1
oy review comment rm 1 --yes
```

The running TUI reloads external review database changes. See [Review commands](./REVIEW.md) for the full review reference.

## Errors

| Error | What to do |
| --- | --- |
| `No Oyo sessions running.` | Start Oyo, then run `oy control list` again |
| `No Oyo session is running for this workspace.` | Start Oyo in this workspace or pass `--session` |
| `More than one Oyo session is running. Pass --session.` | Run `oy control list`, then pass the session name |
| `Failed to connect to Oyo session 'NAME'. Run oy control list.` | Run `oy control list` to remove stale sessions, then retry |
| `No visible diff file matches PATH.` | Check `oy control diff --json`, then use a visible path or change target |
| `More than one visible diff file matches PATH.` | Pass the full path |
| `Specify exactly one navigation target` | Use one of `--new-line`, `--old-line`, `--hunk`, `--step-number`, `--start` or `--end` |
| `Pass a target, --worktree or --staged` | Add one target to `oy control target` |
| `Control queue full. Cancel or wait.` | Run `oy control cancel` or wait for `lastAppliedSeq` |
| `Use on, off or toggle` | Pass one of those values to the mode command |
| `Unknown view mode: MODE` | Use `unified`, `split`, `evolution`, `blame`, `preview`, `next` or `prev` |
| `Unsupported control action: ID` | Use a supported action ID or a named control command |
| `Delay must be a positive duration` | Use a value such as `700ms` or `1s` |
| `Pass a file path` | Pass a path to `oy control file` or `oy control tab file` |
