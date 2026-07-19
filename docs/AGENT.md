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

```markdown
Run `oy skill path`, read the file it prints, then use `oy` and `oy review` for this task.
```

## Workflows

These are the common ways to work with an agent in Oyo.

### Agent reviews code

Use this when you want the agent to inspect a diff and leave feedback.

Example request:

```markdown
Run `oy skill path` and read the file it prints. Review the current change with Oyo. Leave local comments where useful under your agent identity.
```

Name a Git branch, range or jj revset when you do not want the current change reviewed.

### Agent works on comments

Use this when you want the agent to address one comment or work through all unresolved comments.

To share one comment, open its more menu in Oyo and select `Copy id (#N)`, where `N` is the comment number. Include the copied ID in your request:

```markdown
Run `oy skill path` and read the file it prints. Address Oyo comment `#7` in the current Oyo review. Resolve it if you fix it.
```

To share the whole task list, ask the agent to work through all unresolved comments:

```markdown
Run `oy skill path` and read the file it prints. Work through all unresolved comments in the current Oyo review. Resolve the comments you address and report what remains.
```

Oyo excludes outdated comments from the unresolved task list. See [review commands](./REVIEW.md) to inspect outdated comments.

### Agent walks you through code

Use this when you want the agent to explain a live diff one file at a time. Start Oyo before making the request.

Example request:

```markdown
Run `oy skill path walkthrough` and read the file it prints. Use my running Oyo session to walk me through this change one file at a time. Wait for me to say `next`.
```

The agent should use your existing Oyo session. It should only add review comments when you ask.

### Agent controls Oyo

Use this when you want the agent to steer a running Oyo session. It can open files, jump to lines, switch targets or views, and control playback.

Example request:

```markdown
Run `oy skill path control` and read the file it prints. Use my running Oyo session to open `src/lib.rs` and show me line 42.
```

Your keyboard and mouse input take priority over queued agent commands.

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
