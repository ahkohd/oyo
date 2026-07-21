---
name: oyo-code-walkthrough
description: Guide a user through a live Oyo diff one file at a time. Use when the user asks for a code walkthrough, wants the diff explained in Oyo, or wants durable Oyo notes while learning a change.
---

# Oyo code walkthrough

Use this skill for an interactive code walkthrough in a running Oyo TUI.

The user owns the pace and the TUI. Move Oyo to the relevant code, explain one file, then wait for the user to say `next`.

This skill complements the installed `oyo-code-review` and `oyo-tui-control` skills. Read both before starting:

```sh
oy skill path
oy skill path control
```

Read the files returned by those commands. Use the singular command `oy skill path`, not `oy skills path`.

## What a good walkthrough does

A good walkthrough:

- tells one clear story across the diff
- starts with the root cause, not the first file alphabetically
- shows the code in Oyo before explaining it
- explains purpose, ownership and lifecycle
- distinguishes the main fix from mechanical updates
- says what behaviour did not change
- confirms comment mode before the first file when the request does not specify it
- pauses after each file

Do not turn the walkthrough into a code review unless the user asks for one. Do not edit code, commit, push or publish comments without explicit permission.

## Start with the exact Oyo session

List running sessions:

```sh
oy control list --json
```

If the user names a session, use that exact session. Do not continue controlling a different session.

If one matching session exists, select it. If several exist and the user has not chosen one, ask which session to use.

Do not start another interactive Oyo process when the user already has one open.

Check the selected session:

```sh
oy control where --session SESSION --json
oy control diff --session SESSION --json
```

Verify:

- the workspace is correct
- the target is correct
- the file count matches the expected change
- the TUI is showing the intended Git branch, Jujutsu change or pull request

Stop and ask if the target does not match. Do not explain one diff while Oyo shows another.

## Choose comment mode before starting

Before navigating to the first file, confirm whether the walkthrough should:

- remain conversational
- add durable Oyo comments as it progresses

If the request already specifies the mode, do not ask again.

If the user chooses comments, also confirm:

- human or agent authorship
- local-only or published comments

Do not infer authorship or publication from the walkthrough request. Do not start the first file until these choices are clear.

If the user chooses a conversational walkthrough, do not add comments unless they change their mind later.

## Plan the story before moving files

Do not follow alphabetical file order by default. Use this order when it fits the change:

1. core implementation or root cause
2. focused regression test for the core behaviour
3. public type or API contract
4. shared helper that spreads the fix
5. call site that produced the reported symptom
6. remaining production consumers
7. test mocks and compatibility updates
8. verification and end-to-end summary

For a cross-layer bug, explain the boundary first. Examples include preload to renderer, client to server, native to TypeScript, or store to component.

Keep a private list of every changed file. Mark files as covered so you do not skip one or claim the walkthrough is complete too early.

## Walk through one file at a time

Move Oyo before explaining:

```sh
oy control file --session SESSION path/to/file.ts
```

Use a specific line when the key change is not visible:

```sh
oy control goto \
  --session SESSION \
  --file path/to/file.ts \
  --new-line 42
```

A navigation command may be queued. When Oyo returns a sequence number, poll:

```sh
oy control where --session SESSION --json
```

Continue only when `lastAppliedSeq` is at least the queued sequence number.

For each file, explain these points in order:

1. what the file owns
2. what the old code did
3. why the old behaviour failed or became unsafe
4. what the new code does
5. what behaviour remains unchanged
6. how this file connects to the previous file

Use exact event, function and type names. Prefer a short code extract when it makes the change easier to see.

End each file with:

```text
Say `next` when ready.
```

Do not move to the next file until the user asks. This gives them time to inspect the highlighted code and ask questions.

## Explain the bug precisely

Use the most precise name for the problem.

For example, do not call a cross-realm function identity bug an ordinary scope bug. You can acknowledge the user's mental model, then refine it:

```text
Broadly, yes. More precisely, this is a cross-realm function identity bug. The fix uses a closure to retain the exact function registered in the original realm.
```

Separate cause from symptom:

- cause: the identity, lifetime or ownership mistake
- symptom: leaked listeners, duplicate requests, stale state or warnings
- fix: the smallest ownership change that makes cleanup reliable

## Add Oyo comments only in comment mode

Add durable Oyo comments only when the user selected comment mode before the walkthrough or asks for comments later.

Before the first comment, confirm the intended author and destination if they are not already clear:

- use an agent identity for agent-authored review feedback
- use the user's identity only when they explicitly ask
- confirm whether the comments will stay local or be pushed to a provider

Never silently impersonate the user.

You can read the local identity for confirmation:

```sh
git config user.name
git config user.email
gh api user --jq '{name: .name, login: .login}'
```

Navigate to the line before adding the comment. Then create it:

```sh
oy review comment new \
  -t TARGET \
  --file path/to/file.ts \
  --new-line 42 \
  --body "Explain the ownership or lifecycle reason here." \
  --author-type human \
  --author-name "NAME" \
  --author-email "EMAIL" \
  --author-username "github=USERNAME" \
  --json
```

Use `--author-type agent` for agent comments.

A useful walkthrough comment explains one of these:

- why the old code failed
- why the new ownership boundary is safe
- why cleanup belongs at this location
- what regression the test prevents
- which behaviour deliberately remains unchanged

Avoid comments that only restate the diff. One focused comment per important file or concept is usually enough.

## Correct comment authors safely

`oy review comment edit` changes the body, not the author. If the user asks to change authorship, export and reapply the comments with the existing IDs.

Export first:

```sh
oy review export -t TARGET --format json --output /tmp/oyo-comments.json
```

Update the author fields while preserving comment IDs, anchors, bodies and timestamps. Then apply the file:

```sh
oy review comment apply -t TARGET /tmp/oyo-comments-updated.json --json
```

Verify every comment afterwards:

```sh
oy review comment -t TARGET --json
```

Check:

- comment count
- `authorType`
- name and email
- provider username
- target and review key

Do not run `oy review push` unless the user explicitly asks you to publish the comments.

## Handle user interaction correctly

The user's input wins. If they click, type or move Oyo while a command is queued, re-check the current state:

```sh
oy control where --session SESSION --json
```

If the user switches sessions, immediately use the new session name for every later control command.

Do not change their preferred view, sidebar or step mode unless it helps the walkthrough or they ask.

## Use the comments sidebar at the end

After the final file, show the accumulated notes:

```sh
oy control sidebar --session SESSION comments
```

Then confirm the count:

```sh
oy review status -t TARGET --json
```

Explain whether comments are:

- local or published
- human or agent authored
- unresolved or resolved
- explanatory notes or actionable review findings

Do not resolve explanatory comments without asking. In Oyo, unresolved comments normally act as a task list.

## Finish with an end-to-end summary

After every file is covered, summarise the change as a flow rather than repeating each file.

A useful summary covers:

1. where the bug started
2. how the faulty behaviour spread
3. where ownership now lives
4. how callers clean up
5. which tests prevent regression
6. what runtime or CI verification passed

Then offer a small set of next steps, such as:

- review the full fix end to end
- review tests and runtime verification
- inspect all Oyo comments
- resolve walkthrough comments
- finish

## Example response shape

```markdown
Opened:

`path/to/file.ts`

## File 4: shared listener cleanup

This helper owns the common subscription path.

Previously it registered a callback and later tried to remove that callback through a boundary that did not preserve function identity.

It now stores the cleanup function returned at registration time:

```ts
const unsubscribe = api.on(event, handler)
```

Both abort and manual teardown call that same closure. Payload validation and callback behaviour remain unchanged.

Oyo comment `#5` marks this ownership boundary. Say `next` when ready.
```

## Walkthrough checklist

Before starting:

- [ ] read the Oyo review and control skills
- [ ] select the exact user session
- [ ] verify workspace and target
- [ ] confirm conversational or durable comment mode
- [ ] if using comments, confirm author and local or published destination
- [ ] inspect the complete file list
- [ ] plan a root-cause-first order

For every file:

- [ ] navigate before explaining
- [ ] explain ownership and lifecycle
- [ ] distinguish cause, fix and unchanged behaviour
- [ ] add a focused comment when durable comment mode is active
- [ ] stop and wait for `next`

Before finishing:

- [ ] cover every changed file
- [ ] verify comment authors and target
- [ ] open the comments sidebar when useful
- [ ] summarise the end-to-end flow
- [ ] do not push, commit or modify code without permission
