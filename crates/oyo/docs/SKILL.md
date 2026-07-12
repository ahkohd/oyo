---
name: oyo-code-review
description: Use oy to review code, read Oyo comments, or work on comments in Git or jj. Load before reviewing or updating a branch, pull request, jj change, or saved Oyo review.
---

# Oyo code review

Use this skill when you review code with Oyo, or when the user has left Oyo comments for you to work on.

Use `oy` to inspect the diff. Use `oy review` to read or write saved comments.

## Workflow

```text
1. oy review pull --json                    # pull PR comments and verify the target
2. oy review comment --json                 # check reviewKey, label and pr
3. oy review comment --unresolved           # use unresolved comments as the task list
4. oy review comment --id 7                 # read one comment by id
5. edit files and run the smallest useful checks
6. oy review comment reply 7 --body "Fixed." # answer a local or pulled inline thread
7. oy review comment resolve 7              # mark addressed comments resolved
8. oy review status --unresolved            # confirm what remains
9. oy review push                           # publish replies, comments and thread changes
```

## Control a running TUI

Use live control when Oyo is already open and the user wants you to steer it.

Load the control skill for the installed command details:

```sh
oy skill path control
```

Read the file it prints. Then start with:

```sh
oy control list
oy control where --json
```

Use `oy control` only for TUI state. Use `oy review` to create, reply to, edit, resolve or delete comments.

## Choose the target

Run commands from the worktree or workspace under review.

In Git:

```sh
oy                                  # inspect working tree changes
oy review status                    # read the local PR review when one exists
oy review status -t @               # read working tree comments explicitly
oy --staged                         # inspect staged changes
oy --staged review status           # read staged comments
oy feature                          # inspect a branch review
oy --range main...feature           # inspect a pull request-shaped branch review
oy review status -t feature         # read branch comments
oy review status -t main...feature  # read range comments
oy HEAD                             # inspect one commit
```

- all review commands prefer the current branch's existing local PR review
- commands fall back to the working tree when no local PR review exists
- the default is stateless and does not call the provider
- use `-t @` to select the working tree explicitly
- use `-t base...feature` for a pull request-shaped review
- a clean `git status` only means the working tree is clean

In jj:

```sh
oy                         # inspect the default target
oy @                       # inspect the current change
oy feature                 # inspect a bookmark stack
oy 'trunk()..@'            # inspect the current stack
oy review status @         # read current change comments
oy review comment '@-..@'  # read a revset review
```

- review commands default to `@` unless `@` has exactly one bookmark
- explicit `@` always means the current jj change
- multi-change revsets show the latest saved comments for each change ID

Oyo scopes reviews to the current Git worktree or jj workspace.

## Show saved reviews and status

```sh
oy review
oy review log
oy review --json
oy review log --json
oy review status
oy review status --json
oy review status --unresolved
oy review status --outdated
oy review status --no-outdated
oy review status --unresolved --outdated
oy review status --id 7 --json
oy review status --since 1783478786 --json
```

- `oy review` is the same as `oy review log`
- `log` lists saved reviews with comments for the current workspace
- `status` shows a compact comment summary for the current target
- `--json` gives stable output for scripts
- check `reviewKey`, `label` and `pr` before acting on comments
- use `-t/--target` to pin every command to one review
- `--id` is repeatable and narrows output to matching comment IDs
- `--outdated` shows only outdated comments
- `--no-outdated` hides outdated comments
- `--unresolved` excludes outdated comments by default
- `--unresolved --outdated` shows comments in both states
- `commentCount` reflects the active filters
- `--since` returns comments changed at or after the Unix timestamp
- JSON output for `--since` marks outdated-state transitions as `updated` and deletions as `removed`

## Read saved comments

Use `comment` to read full comment bodies:

```sh
oy review comment
oy review comment --json
oy review comment --unresolved
oy review comment --outdated
oy review comment --no-outdated
oy review comment --unresolved --outdated
oy review comment --id 7
oy review comment --id 7 --json
oy review comment --since 1783478786 --json
oy review comment --author-type human
oy review comment --author ada
```

Pin a target when you need deterministic selection:

```sh
oy review comment -t feature
oy review comment -t main...feature
oy review comment -t @
oy review comment -t 'trunk()..@'
```

The positional forms remain available for compatibility.

Use the short saved-review ID from `oy review` when needed:

```sh
oy review
oy review comment a
```

If an ID does not match, Oyo exits non-zero and prints:

```text
No comment matches id 7.
```

## Work on saved comments

Treat unresolved comments as the task list:

```sh
oy review comment --unresolved
```

This task list already excludes outdated comments because stale anchors are not actionable. Use `--unresolved --outdated` to inspect the intersection.

When the diff changes, Oyo re-anchors comments that still match their code. It marks a comment outdated only when the anchored line changed or vanished. Outdated comments are hidden from live inline overlays. Human output shows `Status: unresolved (outdated)` when both states apply.

Press `g o` in the TUI to open the Outdated comments tab. Its cards show the original file and line, comment body and captured anchor snapshot.

Poll for changes when another user or agent may be editing comments:

```sh
oy review comment --since 1783478786 --json
```

Read the full comment bodies, edit the referenced files, run the smallest useful checks, then resolve comments you addressed.

## Add local comments

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

Pass author details when the comment should show an agent or a different local user:

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

Set agent identity once per session when comments should show an agent:

```sh
export OYO_REVIEW_AUTHOR_TYPE=agent
export OYO_REVIEW_AUTHOR_NAME="Agent name"
export OYO_REVIEW_AUTHOR_EMAIL="agent@example.com"
export OYO_REVIEW_AUTHOR_USERNAME=agent
```

Use `--author-email` or repeat `--author-username`. Pass `provider=username` only when you need a provider-specific identity.

## Reply, update or delete comments

Reply to a local or pulled inline comment by its `#id`:

```sh
oy review comment reply 1 --body "Fixed in the latest change."
oy review comment reply -t feature 1 --body "Fixed in the latest change." --json
```

The reply keeps the parent anchor and thread. Replies to local inline comments stay local. `oy review push` publishes replies to pulled provider comments in that review thread. Conversation and file-level comments use their existing actions instead.

Use the `#id` shown by `oy review comment` for edit, resolve and delete commands.

```sh
oy review comment edit 1 --body "Handle empty input before parsing."
oy review comment resolve 1
oy review comment unresolve 1
oy review comment rm 1 --yes
```

Removing a thread root also removes its editable replies. Removing a reply leaves its parent and siblings unchanged.

Use the same target option for every comment action:

```sh
oy review comment edit -t main...feature 1 --body "Handle empty input before parsing."
oy review comment resolve -t main...feature 1
oy review comment rm -t main...feature 1 --yes
```

The positional forms remain available for compatibility.

`reopen` is an alias for `unresolve`. Resolve or reopen the parent comment; replies inherit the thread state and cannot be resolved separately.

## Pull and push pull request comments

Pull remote comments before you work on a pull request:

```sh
oy review pull
oy review pull -t main...feature
oy review pull -t main...feature origin
```

Push local comments when you want to publish them:

```sh
oy review push
oy review push -t main...feature
oy review push -t main...feature origin
```

- provider sync needs matching authentication: `gh` for GitHub, `glab` for each GitLab host, `cb` for Codeberg or `fj` for each self-hosted Forgejo host
- Oyo finds the remote from the current branch upstream, then falls back to `origin`
- pulled provider comment bodies can be read-only
- push sends body changes only for comments you can edit
- push publishes replies to pulled provider inline review threads; local replies stay local
- inline review thread resolve and unresolve changes sync once per thread when the provider API supports it
- Forgejo resolved state is read-only because its API has no review-thread resolve endpoint; resolve attempts warn without blocking other push changes
- pull, read, resolve and push use the same PR-aware default target
- check the target fields in JSON and use `-t` when an agent must pin the review

## Export and apply comments

Export comments to Markdown:

```sh
oy review export
oy review export -t feature --output review.md
```

Export comments to JSON:

```sh
oy review export --format json > comments.json
oy review export -t feature --format json --output comments.json
```

Apply comments from a JSON file:

```sh
oy review comment apply comments.json
oy review comment apply -t feature comments.json
cat comments.json | oy review comment apply -
```

Use this JSON shape for inline comments:

```json
{
  "version": 1,
  "comments": [
    {
      "file": "src/lib.rs",
      "kind": "line",
      "side": "new",
      "newRange": { "start": 42, "end": 42 },
      "author": {
        "name": "Ada Lovelace",
        "email": "ada@example.com",
        "usernames": { "github": "ada" }
      },
      "canEdit": true,
      "resolved": false,
      "createdAt": 1783478786,
      "updatedAt": 1783478786,
      "body": "Handle empty input before parsing."
    }
  ]
}
```

- use `kind: "pr"` for pull request comments
- Oyo assigns an `id` when the comment does not include one
- if a comment includes an existing `id`, Oyo updates that comment
- Oyo stores `createdAt` and `updatedAt` as Unix timestamps in seconds

## Abandon a review

Delete saved review state for the current target:

```sh
oy review abandon
oy review abandon --json
```

Delete saved review state for another target:

```sh
oy review abandon -t feature
oy review abandon -t @
```

## Common errors

- `No comment matches id N.` - check `oy review comment` and use the shown ID
- `Comment N is not an inline comment.` - choose a line or hunk comment
- `Comment N cannot accept replies.` - the comment has provider metadata that cannot accept synced replies
- `Pass --yes to remove a comment` - add `--yes` to confirm deletion
- `Pass --new-line, --old-line or --file-level` - choose where the new comment belongs
- `No valid Forgejo token found for HOST.` - authenticate that host with `cb` for Codeberg or `fj` for self-hosted Forgejo
- `No saved reviews.` - there are no saved comments for the current workspace and target
