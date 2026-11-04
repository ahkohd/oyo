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

Replace the example repository and request number with your own. Public github.com URLs work without signing in and require `curl` for HTTPS reads. GitHub limits unauthenticated API requests per originating IP. Oyo reports rate-limit responses and provider retry or reset information, and does not automatically retry or change identities. Authentication through `gh` remains available for private repositories and higher limits. Other providers require their corresponding CLI login. Oyo uses `gh` for authenticated GitHub reads and all GitHub discussion writes, `glab` for GitLab, credentials saved by `cb` for Codeberg, and host credentials saved by `fj` for Forgejo. Oyo does not ask for a separate access token. Private repositories require access through that account.

## Supported targets

Commit and comparison URLs use these paths after the repository name:

| Provider | Commit | Comparison |
| --- | --- | --- |
| GitHub | `/commit/REV` | `/compare/BASE...HEAD` |
| GitLab | `/-/commit/REV` | `/-/compare/BASE...HEAD` |
| Codeberg and Forgejo | `/commit/REV` | `/compare/BASE...HEAD` |

A commit compares against its first parent, or an empty tree for a root commit. Three-dot comparisons start at the merge base. GitLab, Codeberg and Forgejo also accept `BASE..HEAD` to compare the endpoints directly. GitHub two-dot comparisons and comparison query options are unsupported.

## Loading and refresh

Oyo opens a loading screen while it resolves the remote snapshot. Press `q`, `Esc` or `ctrl-c` to cancel startup. If startup fails, press `r` to retry.

Public github.com review URLs stream changed lines over HTTPS. Authenticated GitHub review URLs stream when the installed `gh api` supports `--allow-escape-sequences`. Oyo uses that flag only on a private parser pipe and escapes terminal controls in displayed text. The first parsed changed file becomes readable while the file list grows. The status marks the manifest incomplete until the entire response and a second provider identity check succeed. Readable partial content is for inspection; comments, reviewed marks and provider writes wait for validation.

Patch context is distinct from complete file contents. Press `F`, expand context or open preview to load the selected file at its fixed revisions. All-file search requests missing full contents and marks results incomplete while data or snapshot validation is unavailable. A failed download keeps the readable patch and waits for an explicit retry. Hydration retains the logical cursor, viewport and saved drafts. An active text selection is cleared with a notice when the layout changes; select the text again before copying or commenting.

GitLab, Codeberg, Forgejo, commit and comparison URLs use complete-content loading. Older authenticated GitHub CLIs also use that path. Configured GitHub credentials stay on the CLI route even if they fail validation; an authentication or quota failure does not trigger an anonymous retry. Pass `--remote-full` to choose it explicitly, including when a patch exceeds the streaming limits. The complete-content path retains the provider enumeration limits below.

Press `R` to resolve a new snapshot. A failed refresh keeps the current view. A successful refresh after an initial stream failure also restores saved review setup. Saved reviews remain available when you open the same URL from another directory.

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

GitHub streaming review manifests come from the complete pinned patch rather than the REST changed-files list. Oyo checks unique paths, file count, additions and deletions against review metadata before marking the snapshot complete. Metadata-only changes, empty files, renames, binary records, symlinks and mode changes remain manifest entries. GitHub complete-content review enumeration rejects lists beyond 3,000 files. GitHub commit enumeration stops at 3,000 files, and comparisons stop at 300 files. Oyo rejects those results because the provider may have truncated them. GitLab reviews require an exact changed-file count.

A streamed patch is limited to 48 MiB in total, 32 MiB per file record or line, 250,000 lines per file record and 100,000 files. Malformed, combined, truncated and over-limit patches produce an explicit incomplete state. Git paths containing control characters, backslashes or traversal components are rejected. Use `--remote-full` when a supported complete-content path can handle a patch outside these bounds.

Review enumeration stops after 100 pages and tree enumeration after 1,000 pages. Forgejo merge-base searches stop after 2,048 commits and reject missing or multiple merge bases. Use a two-dot comparison when you need a direct endpoint comparison.

Individual file downloads are limited to 32 MiB. Each transport request has a 30-second deadline. Binary and oversized files display explicit states. Saved full-file comment captures retain the 256 KiB per-side limit; larger files may lack a full outdated captured view.
