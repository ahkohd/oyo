# Working with agents

Use this page when a human and an AI agent work in the same Oyo review.

Oyo uses a Git-shaped and jj-shaped CLI, so humans and agents can use the same targets and commands. Most coding agents already understand Git commands. Oyo builds on that model instead of adding a new review workflow.

## Start with the Oyo skill

Ask the agent to read the installed Oyo skill first:

```sh
oy skill path
```

Tell the agent to read the file path printed by that command. This uses the skill for the installed `oy` version.

If you are reading from the repo, use [the SKILL.md file](../crates/oyo/docs/SKILL.md).

Example instruction:

```text
Run oy skill path, read the file it prints, then use oy and oy review for this task.
```

## Workflows

These are the common ways to work with an agent in Oyo.

### Agent reviews code

Use this when you want the agent to inspect a diff and leave feedback.

Ask the agent to:

1. read the Oyo skill
2. confirm the review target
3. open the diff with `oy`, `oy --staged`, `oy --range main...HEAD` or the jj target
4. set agent identity with `OYO_REVIEW_AUTHOR_TYPE=agent`
5. add comments with `oy review comment new` when useful
6. report the target reviewed and the checks run

Example agent identity and comment:

```sh
export OYO_REVIEW_AUTHOR_TYPE=agent
export OYO_REVIEW_AUTHOR_NAME="Agent name"
oy review comment new \
  --file src/lib.rs \
  --new-line 42 \
  --body "Handle empty input."
```

### Agent works on comments

Use this when you have left Oyo comments and want the agent to respond or make changes.

Ask the agent to:

1. read the Oyo skill
2. run `oy review status` to see the comment summary
3. run `oy review comment --unresolved` to read full comment bodies
4. treat the returned comments as the task list; this filter excludes outdated comments
5. edit the referenced files
6. run the smallest useful checks
7. run `oy review comment resolve <id>` for comments you addressed
8. report what changed and which comments are still unresolved

Oyo marks a comment outdated when its anchored line changed or vanished as the diff evolved. It hides outdated comments from live inline overlays. Re-anchored comments follow their code without becoming outdated.

`--unresolved` excludes outdated comments by default because stale anchors are not actionable. Agents can inspect stale unresolved comments with:

```sh
oy review status --unresolved --outdated
oy review comment --unresolved --outdated
```

Use `--outdated` alone to show only outdated comments. Use `--no-outdated` to hide them explicitly. Filtered JSON counts only matching comments. With `--since`, an outdated-state transition has `changeType: "updated"` and a deleted comment has `changeType: "removed"`.

## Pick the right target

For Git working tree reviews, use:

```sh
oy
oy review status
oy review comment
```

For Git branch reviews, use:

```sh
oy feature
oy --range main...HEAD
oy review status feature
oy review comment feature
oy review status development...feature
```

A clean `git status` only means there is no uncommitted work. It does not mean the branch has no committed changes.

For jj reviews, use:

```sh
oy @
oy feature
oy 'trunk()..@'
```

Use `@` for the current change, a bookmark name for a stack, or a revset for an exact range.

## Sync remote comments

Agents can pull remote comments when the review needs pull request state and the local `gh` account is authenticated:

```sh
oy review pull
```

`oy review push` publishes local comments to the pull request:

```sh
oy review push
```

Commit, push, delete comments and `oy review push` change local or remote state.

## Use JSON for tools

Use JSON output when another tool will parse the result:

```sh
oy review status --json
oy review comment --json
```
