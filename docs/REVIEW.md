# Review commands

Use `oy review` from the Git worktree or jj workspace you are reviewing.

Oyo resolves the current workspace and review target. It then reads or writes the saved review for that target.

## Saved review database

Oyo saves review state in one SQLite database per workspace:

```text
<data>/oyo/reviews/<workspace>/review.db
```

Use `--json` when a script needs stable output.

## Choose where Oyo saves the database

By default, Oyo uses the platform app data directory.

Set a different directory in config:

```toml
[review]
dir = ".oyo/reviews"
```

Oyo uses these path rules:

- unset `review.dir` uses the platform app data directory
- relative `review.dir` resolves from the current workspace root
- absolute `review.dir` is used as-is
- `--review-dir` overrides config

Set a different directory for one run:

```sh
oy --review-dir .oyo/reviews
```

Disable persisted review state:

```sh
oy --no-review-persist
```

## Show reviews

`oy review` is the same as `oy review log`.

```sh
oy review
oy review log
oy review --json
oy review log --json
```

`log` lists saved reviews for the current target. It hides saved reviews with no comments.

Use `status` to show comments for the current target:

```sh
oy review status
oy review status --json
```

Plain `status` hides the internal database path and shows comment IDs. `status --json` includes the database path and stable fields for scripts:

```json
{
  "workspaceRoot": "/repo-worktree",
  "target": "@  feature",
  "label": "@  feature",
  "pr": null,
  "diffFingerprint": "abc123",
  "reviewKey": "jj:change:...",
  "reviewDir": "...",
  "reviewDb": ".../review.db",
  "commentCount": 3,
  "comments": [
    {
      "id": 1,
      "subject": "src/lib.rs",
      "location": "R42",
      "preview": "Handle empty input."
    }
  ]
}
```

## Use Git and jj review targets

A review target follows the current VCS.

In Git, every review command prefers the current branch's existing local pull request review. It falls back to the working tree:

```sh
oy review status
oy review comment
oy review status -t @
oy review comment -t @
```

Oyo derives the default from the current branch and local review database. It does not store an active target or contact the provider.

Use staged changes:

```sh
oy --staged
oy --staged review status
```

Use a commit, branch, ref, commit hash or range:

```sh
oy feature
oy --range main...feature
oy review status -t HEAD
oy review status -t feature
oy review status -t a1b2c3d
oy review status -t main..feature
oy review status -t main...feature
oy review status -t development...feature
oy review comment -t development...feature
```

Use `base...feature` for a pull request-shaped review. The base can be any branch. The 3-dot form compares the feature branch from its merge base with the base branch.

Oyo stores the branch label and the resolved commits. If the branch moves, Oyo can still load the latest saved comments for that branch review.

Pull, read, resolve and push use the same PR-aware default. Use `--json` to check `reviewKey`, `label` and `pr`. Use `-t/--target` to pin a target for scripts and agents.

In jj, review commands and `oy` default to `@` unless `@` has exactly one bookmark. If `@` has one bookmark, Oyo treats that bookmark like a branch and opens the bookmark stack.

```sh
oy
oy review status
oy review comment
```

Force the current jj change with `@`:

```sh
oy @
oy review status @
oy review comment @
```

Use a bookmark like a Git branch:

```sh
oy feature
oy review status feature
oy review comment feature
```

Use a change ID, commit ID or revset when you need an exact jj target:

```sh
oy znkkqopx
oy '@-..@'
oy 'trunk()..@'
oy review status @-
oy review status 'trunk()..@'
oy review comment 'trunk()..@'
```

For multi-change jj revsets, Oyo expands the revset and shows the latest saved comments for each change ID.

Branches, bookmarks and worktrees are labels, not diff fingerprints. Oyo stores reviews under a stable review key, such as a jj change ID, Git branch target or Git worktree root.

## Worktrees and workspaces

Oyo scopes reviews to the workspace you run it from.

For Git, this means the current worktree root. Two Git worktrees that share a repository are separate review contexts.

For jj, this means the current workspace root. Two jj workspaces are separate review contexts.

Relative review directories also resolve from that workspace root.

```sh
cd ~/repo-feature
oy review status
```

This shows the review for `~/repo-feature`, not another worktree.

## Use the TUI for a review target

Open Oyo with the default diff target:

```sh
oy
```

Open a Git branch, jj bookmark or jj revset:

```sh
oy feature
oy --range main...feature
oy 'trunk()..@'
```

When you pass a target, the TUI uses the same target rules as the CLI. Saved comments load from the stable review key, even when the current diff fingerprint has changed.

Review cards in unified and split mode show `ia edit`, `ra reply`, `xa delete` and so on. Thread roots also show `va resolve`; reply cards do not because resolved state belongs to the thread. Every inline comment can have replies. Local replies stay in Oyo; replies to pulled provider comments sync to their provider thread. Replies render as a flat chronological thread, linked by a connector between cards. Resolved root cards show `unresolve` instead of `resolve`. Pull request conversation cards do not show resolve actions.

Use the comments sidebar to read and move through comments. Use the comment picker to search comments and jump to one:

```sh
ctrl-shift-c
```

In normal mode you can also press `g c`.

Use the sidebar overflow menu to pull or push remote comments. If there is more than one remote, Oyo opens a remote picker. The footer shows sync progress while pull or push runs.

## Comments

Use `comment` to read full comment bodies for the current target:

```sh
oy review comment
oy review comment --json
```

Pass a target to read comments for a branch, bookmark, commit, change ID or range:

```sh
oy review comment feature
oy review comment main...feature
oy review comment @
oy review comment 'trunk()..@'
```

Use the short ID from `oy review` to open a saved review:

```sh
oy review
oy review comment a
```

Add a line comment:

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

Use `--author-email` or repeat `--author-username`. Pass `provider=username` only when you need a provider-specific identity.

Agents can set their identity once per session instead of passing flags every time.

```sh
export OYO_REVIEW_AUTHOR_TYPE=agent
export OYO_REVIEW_AUTHOR_NAME="Agent name"
export OYO_REVIEW_AUTHOR_EMAIL="agent@example.com"
export OYO_REVIEW_AUTHOR_USERNAME=agent
```

Filter the task list:

```sh
oy review status --unresolved
oy review status --outdated
oy review status --no-outdated
oy review status --unresolved --outdated
oy review status --id 7 --json
oy review status --since 1783478786 --json
oy review comment --unresolved
oy review comment --outdated
oy review comment --no-outdated
oy review comment --unresolved --outdated
oy review comment --id 7
oy review comment --since 1783478786 --json
oy review comment --author-type human
oy review comment --author ada
```

### Understand outdated comments

When the diff changes, Oyo tries to move each comment with its anchored code. A comment follows the code without a warning when Oyo can re-anchor it safely. Oyo marks the comment outdated only when the anchored line changed or vanished. Outdated comments are hidden from live inline overlays.

Both `status` and `comment` support these filters:

- `--outdated` shows only outdated comments
- `--no-outdated` hides outdated comments
- `--unresolved` excludes outdated comments by default because a stale anchor is not actionable
- `--unresolved --outdated` shows comments that are both unresolved and outdated

`commentCount` reflects the active filters. Human comment blocks show `Status: unresolved (outdated)` when both states apply.

Press `g o` or choose Outdated comments from the command palette to open the dedicated tab. Clicking an outdated comment in the sidebar opens the same tab and focuses that comment. Each card shows the original file and line, comment body and captured anchor snapshot. You can edit, resolve, reopen or delete comments from this view.

`--since` returns comments changed at or after the Unix timestamp. JSON output includes `changeType` as `added`, `updated` or `removed`. An outdated-state transition appears as `updated`. A deleted comment appears as `removed`.

Reply to a local or pulled inline comment:

```sh
oy review comment reply 1 --body "Fixed in the latest change."
oy review comment reply -t feature 1 --body "Fixed in the latest change." --json
```

The new comment keeps the parent anchor and thread. Replies to local inline comments stay local. `oy review push` posts replies to pulled GitHub comments in that review thread. Conversation and file-level comments use their existing actions instead.

Edit or remove a comment:

```sh
oy review comment edit 1 --body "Handle empty input before parsing."
oy review comment rm 1 --yes
```

Pass a target when the comment belongs to a saved branch, bookmark or range review:

```sh
oy review comment edit main...feature 1 --body "Handle empty input before parsing."
oy review comment rm main...feature 1 --yes
```

Removing a thread root also removes its editable replies. Oyo asks for confirmation in the TUI and reports the reply count in the CLI. Removing a reply leaves its parent and siblings unchanged.

Resolve or unresolve a comment:

```sh
oy review comment resolve 1
oy review comment unresolve 1
```

`reopen` is kept as an alias for agents that already use it. Resolve or reopen the parent comment; replies inherit the thread state and cannot be resolved separately.

## Push and pull pull request comments

Push and pull sync the local review with the matching pull request.

Oyo currently supports GitHub through `gh`. Install and authenticate `gh` before you run these commands.

`oy review status`, `oy review log`, `oy review comment` and the TUI read the local database. Remote comments appear after you pull them.

Oyo finds the remote from the current branch upstream, then falls back to `origin`. Pass a remote name when you want another remote.

Use `pull` to bring pull request comments into Oyo:

```sh
oy review pull
oy review pull main...feature
oy review pull main...feature origin
```

In Git, `pull` resolves the current branch or the range head to the matching pull request. In jj, use a bookmark for a branch-like review target.

Use `push` to send your local comments to the pull request:

```sh
oy review push
oy review push main...feature
oy review push main...feature origin
```

Push and pull use the matching pull request. If Oyo cannot find one, it stops with an error.

GitHub has a built-in `gh` adapter. GitLab, Codeberg and Forgejo adapters are planned.

Custom providers use the provider command contract in config.

### Provider command contract

Oyo owns the provider interface. Provider tools return Oyo-shaped JSON.

Built-in adapters can call provider CLIs and APIs directly. Custom providers are configured under `[review.providers.<id>]`.

Minimum provider commands are:

- `whoami`
- `pr_find`
- `pr_get`
- `comments_list`
- `comments_create`
- `comments_update`
- `comments_delete`

`whoami` returns the authenticated provider user:

```json
{
  "username": "ada",
  "name": "Ada Lovelace",
  "avatarUrl": "https://example.com/ada.png"
}
```

`pr_get` returns pull request metadata:

```json
{
  "provider": "example",
  "remote": "origin",
  "repo": "owner/name",
  "number": 123,
  "title": "Add parser",
  "url": "https://git.example.com/owner/name/pulls/123",
  "baseBranch": "main",
  "headBranch": "feature",
  "baseCommit": "abc",
  "headCommit": "def"
}
```

`comments_list` returns comments:

```json
{
  "comments": [
    {
      "providerCommentId": "123",
      "providerThreadId": "456",
      "author": {
        "name": "Ada Lovelace",
        "username": "ada",
        "usernames": { "example": "ada" },
        "avatarUrl": "https://example.com/ada.png"
      },
      "file": "src/lib.rs",
      "kind": "line",
      "side": "new",
      "newRange": { "start": 42, "end": 42 },
      "body": "Handle empty input.",
      "createdAt": "2026-07-08T12:34:56Z",
      "updatedAt": "2026-07-08T12:34:56Z",
      "canEdit": false
    }
  ]
}
```

Mutation commands read the same Oyo-shaped JSON from standard input.

See [provider config](./CONFIG.md#provider-command-interface).

### What `pull` imports

Oyo imports these comments by default:

- all inline review comments on files and lines
- pull request conversation comments from you
- pull request conversation comments from the PR author
- pull request conversation comments from requested reviewers
- pull request conversation comments from people who have submitted a review

Oyo skips pull request conversation comments from other users. This keeps bot and drive-by comments out of the local review unless the account is part of the review set.

Other users' comment bodies are read-only. GitHub inline review comments also
import their review thread's resolved state.

### What `push` sends

Oyo sends comment changes only for comments you can edit.

For your comments, push can:

- create new inline comments
- reply in local inline threads and pulled GitHub inline review threads
- create new pull request comments
- update comments that already exist on the provider
- delete comments you removed locally
- resolve or unresolve linked GitHub inline review threads

Resolve changes are sent once per review thread. Pull request conversation comments
and local-only comments keep their resolved state locally because they have no
provider review thread.

### Pull request and merge request comments in the TUI

Oyo uses PR and pull request for GitHub and Forgejo. It uses MR and merge request for GitLab.

Pull request or merge request comments appear in the comments sidebar with this format:

```text
Bob, 10s ago - Pull request title
```

Click a pull request comment to open the pull request comments view. Oyo does not show these comments inside file diffs.

The pull request comments view lists comments in time order. Use `m` or the add row at the end to add a new pull request comment. Oyo only shows the add row when the current review is linked to a pull request.

Editable comments show `ia edit`, `ib edit` and so on. They also show `xa delete`, `xb delete` and so on. Press or click the edit action, or click the card, to edit the comment. Read-only comments do not show edit or delete actions.

Use the reply action under a pull request conversation comment to quote it in a new comment. Oyo uses Markdown blockquotes for the quoted text.

Every inline comment shows a separate `ra reply` action. This opens an empty reply editor and keeps the saved reply in a flat chronological thread under its parent. Local replies stay local. Replies to pulled GitHub review-thread comments sync into the same provider thread.

## Export comments

Export comments to Markdown:

```sh
oy review export
oy review export feature --output review.md
```

Export comments to JSON:

```sh
oy review export --format json > comments.json
oy review export feature --format json --output comments.json
```

## Apply comments

Use `apply` to add or update comments from a file:

```sh
oy review comment apply comments.json
```

Use `-` to read JSON from standard input:

```sh
cat comments.json | oy review comment apply -
```

Use a revision before the file when the comments belong to a specific target:

```sh
oy review comment apply feature comments.json
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
        "usernames": {
          "github": "ada"
        }
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

Use `kind: "pr"` for pull request comments. Pull request comments do not need a file or line anchor when Oyo creates them from the TUI.

Oyo assigns an `id` when the comment does not include one. If a comment includes an existing `id`, Oyo updates that comment.

Oyo adds `author` from Git or jj config when a new comment does not include one. It uses `user.name`, `user.email`, `github.user` and `usernames.<provider>`.

Oyo stores `createdAt` and `updatedAt` as Unix timestamps in seconds. Pulled provider comments also include provider sync data.

## Abandon a review

Use `abandon` to delete the saved review for the current target:

```sh
oy review abandon
```

Abandon a saved review for a specific target:

```sh
oy review abandon feature
oy review abandon @
```

Use `--json` for script output:

```sh
oy review abandon --json
```

