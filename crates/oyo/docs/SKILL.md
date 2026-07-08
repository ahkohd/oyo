---
name: oyo-code-review
description: Use oy to review code, read Oyo comments, or work on comments in Git or jj. Load before reviewing or updating a branch, pull request, jj change, or saved Oyo review.
---

# Oyo code review

Use this skill when you review code with Oyo, or when the user has left Oyo comments for you to work on.

Use `oy` to inspect the diff. Use `oy review` to read or write saved comments.

For command details, see [Review commands](./REVIEW.md).

## Choose the target

Run commands from the worktree or workspace under review.

In Git:

- use `oy` for uncommitted work
- use `oy review status` for uncommitted work comments
- use `oy --staged` for staged work
- use `oy --range main...HEAD` for the current branch review
- use `oy feature` or `oy --range main...feature` for a named branch
- use `oy review status feature` for named branch comments
- use `oy review status development...feature` when the base is not `main`
- use `oy HEAD` for one commit

A clean `git status` only means the working tree is clean. It does not mean the branch has no committed changes.

In jj:

- use `oy` for the default target
- use `oy @` for the current change
- use `oy feature` for a bookmark stack
- use `oy 'trunk()..@'` for the current stack

## Read saved comments

Start with the local review state:

```sh
oy review
oy review status
oy review comment
```

Use `oy review status` for a summary. Use `oy review comment` for full comment bodies.

Use JSON when a tool needs stable output:

```sh
oy review status --json
oy review comment --json
```

Remote comments appear after a pull.

## Work on saved comments

When the user asks you to work on comments, treat `oy review comment` as the task list.

Read the full comment bodies, edit the referenced files, then run the smallest useful checks.

## Sync remote comments

Pull remote comments when you need comments from the pull request:

```sh
oy review pull
```

Push local comments to publish them:

```sh
oy review push
```

Oyo uses `gh` for GitHub. The account must already be authenticated.

## Add local comments

Use local comments to leave review feedback.

Add a new-side line comment:

```sh
oy review comment new --file src/lib.rs --new-line 42 --body "Handle empty input."
```

Add an old-side line comment:

```sh
oy review comment new --file src/lib.rs --old-line 40 --body "This removal changes behaviour."
```

Add a file-level comment:

```sh
oy review comment new --file assets/logo.png --file-level --body "Check this asset size."
```

Pass `--author-type agent` when the comment should show an agent:

```sh
oy review comment new \
  --file src/lib.rs \
  --new-line 42 \
  --body "Handle empty input." \
  --author-type agent \
  --author-name "Agent name" \
  --author-email "agent@example.com" \
  --author-username agent
```

## Update or delete local comments

Use `oy review comment` to get comment IDs.

Edit a comment:

```sh
oy review comment edit 1 --body "Handle empty input before parsing."
```

Delete a comment:

```sh
oy review comment rm 1 --yes
```

Pass a target before the ID when the comment belongs to another review target:

```sh
oy review comment edit main...feature 1 --body "Handle empty input before parsing."
oy review comment rm main...feature 1 --yes
```
