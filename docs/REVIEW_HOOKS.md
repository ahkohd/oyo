# Review hooks

Use review hooks to run your own commands when review comments change, or when the final review is ready.

Oyo saves review state in SQLite. Use `oy review export` when a hook or script needs a file.

Oyo sends a JSON payload to hooks by default. This lets you connect Oyo to other tools without adding those tools to Oyo.

## Choose a hook or an action

Use a hook when Oyo should run the command automatically.

Use an action when you want to run the command yourself from the UI. Actions can appear in the review editor footer and in the command palette.

## Events

| Event | When it runs |
| --- | --- |
| `comment_saved` | A line or hunk comment is saved or updated |
| `comment_deleted` | A comment is removed |
| `comments_cleared` | All comments are cleared |
| `review_ready` | You quit and review output is ready |

## Add an automatic hook

```toml
[[review.hooks]]
id = "archive-review"
on = "review_ready"
command = ".oyo/hooks/archive-review"
stdin = "json"
blocking = true
timeout_ms = 30000
```

This runs `.oyo/hooks/archive-review` when review output is ready.

`command` is the executable to run. It is not a shell string. Put flags and other arguments in `args`:

```toml
command = ".oyo/hooks/archive-review"
args = ["--format", "json"]
```

Do not write this:

```toml
command = ".oyo/hooks/archive-review --format json"
```

If you need shell features, run a shell yourself:

```toml
command = "sh"
args = ["-c", ".oyo/hooks/archive-review --format json"]
```

## Hook fields

| Field | Meaning |
| --- | --- |
| `id` | Name used in warning messages |
| `on` | Event that runs the hook |
| `command` | Executable to run directly |
| `args` | Arguments passed to the command |
| `stdin` | `json` sends the payload, `none` sends nothing |
| `blocking` | `true` waits for the command to finish |
| `timeout_ms` | Maximum wait time when `blocking = true` |

Most hooks only need `id`, `on` and `command`.

## Add a review action

Use an action when you want a visible command.

```toml
[[review.actions]]
id = "save-review-json"
label = "Save review"
key = "ctrl-r"
on = "review_ready"
command = ".oyo/hooks/save-review"
args = ["reviews/latest.json"]
stdin = "json"
blocking = true
timeout_ms = 30000
save_editor = true
show = ["review_editor", "command_palette"]
```

This adds a `ctrl-r` action in the review editor. It can also appear in the command palette.

## Action fields

| Field | Meaning |
| --- | --- |
| `id` | Stable action name |
| `label` | Text shown in the UI |
| `key` | Optional key handled in the review editor |
| `on` | Event name included in the JSON payload |
| `command` | Executable to run directly |
| `args` | Arguments passed to the command |
| `stdin` | `json` sends the payload, `none` sends nothing |
| `blocking` | `true` waits for the command to finish |
| `timeout_ms` | Maximum wait time when `blocking = true` |
| `save_editor` | Save active editor text before running |
| `show` | Use `review_editor`, `command_palette`, or both |

## Review database and commands

Read [review commands](./REVIEW.md) for `oy review`, workspace support, Git and jj targets.

## What Oyo sends

Oyo sends this shape when `stdin = "json"`:

```json
{
  "version": 1,
  "event": "review_ready",
  "repoRoot": "/repo",
  "reviewDb": "/home/me/.local/share/oyo/reviews/.../review.db",
  "diffFingerprint": "abc",
  "diff": {
    "branch": "feature",
    "range": ["main", "HEAD"],
    "files": ["src/lib.rs"]
  },
  "review": {
    "text": "=== Comment 1 ===\n...",
    "comments": [
      {
        "id": 1,
        "file": "src/lib.rs",
        "kind": "line",
        "side": "new",
        "oldRange": null,
        "newRange": { "start": 42, "end": 42 },
        "author": {
          "name": "Ada Lovelace",
          "email": "ada@example.com",
          "usernames": {
            "github": "ada"
          }
        },
        "resolved": false,
        "createdAt": 1783478786,
        "updatedAt": 1783478786,
        "body": "Please fix this."
      }
    ]
  }
}
```

The payload is versioned. Check `version` before you depend on the payload shape. Oyo will bump the version if it makes a breaking payload change.

`side` is `old` or `new`. Hunk comments may include both `oldRange` and `newRange`.

Oyo also sets these environment variables:

- `OYO_REVIEW_EVENT`
- `OYO_REPO_ROOT`
- `OYO_DIFF_FINGERPRINT`
- `OYO_REVIEW_DB`

Hooks run from the repo root when Oyo knows it.

## Blocking and warnings

If `blocking = true`, Oyo waits for the command to finish. It reports non-zero exits and timeouts after the terminal UI exits.

If `blocking = false`, Oyo starts the command and returns immediately. Use this only for fire-and-forget work. Oyo does not report the command exit status.

Oyo hides hook stdout and stderr.

## Start with a small script

Start with a script that writes the payload to a file:

```sh
#!/bin/sh
set -eu

out="${1:-review.json}"
mkdir -p "$(dirname "$out")"
cat > "$out"
```

Then configure an action:

```toml
[[review.actions]]
id = "save-review-json"
label = "Save review"
key = "ctrl-r"
on = "review_ready"
command = ".oyo/hooks/save-review"
args = ["reviews/latest.json"]
stdin = "json"
blocking = true
show = ["review_editor", "command_palette"]
```

Use an automatic hook after the script works as expected.
