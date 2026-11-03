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

Oyo opens a loading screen while it resolves the remote snapshot and saved review. Press `q`, `Esc` or `ctrl-c` to cancel startup. If startup fails, press `r` to retry. Once the file list appears, Oyo loads complete file contents concurrently and gives the selected file priority. Contents stay tied to fixed revisions. Press `R` to resolve a new snapshot. A failed refresh keeps the current view. Saved reviews remain available when you open the same URL from another directory.

## Saved reviews

Use the URL as the review target from any directory:

```sh
oy review status -t https://github.com/owner/repo/pull/123
oy review comment -t https://github.com/owner/repo/pull/123
```

Saved state is keyed by the canonical remote target. Comments retain the revisions they were created against when the remote head changes. Use the same review storage directory when reopening a review. The default app data directory works across working directories; a relative `--review-dir` resolves from the current directory for remote targets.

See [review commands](./REVIEW.md) for comment, export and import operations.

## Sync review discussions

Open the comments sidebar and use its overflow menu to pull or push. Pull imports provider threads and their replies. You can reply, save with `ctrl-s` and push through the same authentication used to load the diff. Repeated pulls update existing comments without importing duplicates.

The CLI accepts the review URL too:

```sh
oy review pull -t https://github.com/owner/repo/pull/123
oy review comment reply 1 --body "Checked this change." -t https://github.com/owner/repo/pull/123
oy review push -t https://github.com/owner/repo/pull/123
```

GitHub and GitLab support resolving and reopening threads. Forgejo and Codeberg expose resolved state as read-only. Threads whose original file or line is unavailable remain in the outdated comments view. Replies use their provider thread identity. Forgejo still requires a usable position from the provider to publish a reply; otherwise the draft stays local with an error.

Only review URLs identify a provider review. Commit and comparison URLs cannot pull or push discussions, and Oyo does not select a pull request for them.

### Changed revisions and failed writes

Oyo checks the displayed head, base and repository identities against the provider before each write. Pending changes retain the revision reviewed when they were saved. Text edits and resolution changes retain separate reviewed revisions. If a revision changes, refresh and review the new diff, then save the pending text or toggle the pending resolution again. New inline comments also require an anchor captured from that displayed snapshot. Older anchors keep their original snapshot.

Publishing requires persistent review storage. Successful creates have durable receipts so an interrupted session can recover their provider IDs. A partial failure keeps successful changes and leaves failed changes pending. An explicit provider rejection can be retried after correcting its cause.

If a connection fails after a create may have reached the provider, Oyo blocks automatic reposting. Pull and inspect the provider. If the comment exists, remove the unsent local draft and keep the imported comment. If it does not exist, remove and recreate the draft before pushing. This avoids treating an uncertain response as a failed write that is safe to repeat.

Revision checks precede API requests. Providers do not offer an atomic revision condition for every mutation, so a provider can still change between the check and the write.

## Limits

Checkout history and remote image previews are unavailable in remote URL sessions. Direct URL launches render review avatars with terminal text blocks to avoid a blocking terminal capability query. Opening a local target restores automatic image protocol detection. File-count and download limits produce explicit errors instead of partial diffs. Tree-based comparisons reject submodules and file-mode changes, and show renames as deletions and additions. Review URLs preserve provider rename information.

GitHub commit enumeration stops at 3,000 files, and comparisons stop at 300 files. Oyo rejects those results because the provider may have truncated them. GitLab reviews require an exact changed-file count.

Review enumeration stops after 100 pages and tree enumeration after 1,000 pages. Forgejo merge-base searches stop after 2,048 commits and reject missing or multiple merge bases. Use a two-dot comparison when you need a direct endpoint comparison.

Individual file downloads are limited to 32 MiB. Each transport request has a 30-second deadline. Binary and oversized files display explicit states. Saved full-file comment captures retain the 256 KiB per-side limit; larger files may lack a full outdated captured view.
