# Remote diffs

## Open a URL

Pass an HTTPS review, commit or comparison URL. You can run Oyo from any directory without a local checkout.

```sh
oy https://github.com/owner/repo/pull/123
oy https://gitlab.com/group/project/-/merge_requests/123
oy https://codeberg.org/owner/repo/pulls/123
oy https://forgejo.example.org/owner/repo/pulls/123
```

## Authentication

Replace the example repository and request number with your own. Authenticate the corresponding CLI for the target host first. Oyo uses `gh` for GitHub, `glab` for GitLab, credentials saved by `cb` for Codeberg, and host credentials saved by `fj` for Forgejo. Oyo does not ask for a separate access token. Private repositories require access through that account.

## Supported targets

Commit and comparison URLs use these paths after the repository name:

| Provider | Commit | Comparison |
| --- | --- | --- |
| GitHub | `/commit/REV` | `/compare/BASE...HEAD` |
| GitLab | `/-/commit/REV` | `/-/compare/BASE...HEAD` |
| Codeberg and Forgejo | `/commit/REV` | `/compare/BASE...HEAD` |

A commit compares against its first parent, or an empty tree for a root commit. Three-dot comparisons start at the merge base. GitLab, Codeberg and Forgejo also accept `BASE..HEAD` to compare the endpoints directly. GitHub two-dot comparisons and comparison query options are unsupported.

## Loading and refresh

Oyo loads complete file contents in the background and keeps them tied to fixed revisions. Press `R` to resolve a new snapshot. A failed refresh keeps the current view. Saved reviews remain available when you open the same URL from another directory.

## Saved reviews

Use the URL as the review target from any directory:

```sh
oy review status -t https://github.com/owner/repo/pull/123
oy review comment -t https://github.com/owner/repo/pull/123
```

Saved state is keyed by the canonical remote target. Comments retain the revisions they were created against when the remote head changes. Use the same review storage directory when reopening a review. The default app data directory works across working directories; a relative `--review-dir` resolves from the current directory for remote targets.

See [review commands](./REVIEW.md) for comment, export and import operations.

## Limits

Remote URL sessions support local review comments, but provider comment synchronization, checkout history and remote image previews are unavailable. File-count and download limits produce explicit errors instead of partial diffs. Tree-based comparisons reject submodules and file-mode changes, and show renames as deletions and additions. Review URLs preserve provider rename information.

GitHub commit enumeration stops at 3,000 files, and comparisons stop at 300 files. Oyo rejects those results because the provider may have truncated them. GitLab reviews require an exact changed-file count.

Review enumeration stops after 100 pages and tree enumeration after 1,000 pages. Forgejo merge-base searches stop after 2,048 commits and reject missing or multiple merge bases. Use a two-dot comparison when you need a direct endpoint comparison.

Individual file downloads are limited to 32 MiB. Each transport request has a 30-second deadline. Binary and oversized files display explicit states. Saved full-file comment captures retain the 256 KiB per-side limit; larger files may lack a full outdated captured view.
