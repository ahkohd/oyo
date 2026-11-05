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

Replace the example repository and request number with your own. Public review URLs on GitHub, GitLab, Codeberg and Forgejo work without signing in when the host permits anonymous repository access. Anonymous HTTPS reads require `curl`. GitHub limits unauthenticated API requests per originating IP. Oyo reports rate-limit responses and provider retry or reset information, and does not automatically retry or change identities. Authentication through `gh` remains available for private repositories and higher limits. Configured accounts retain their authenticated API route. Invalid credentials and access or quota errors do not trigger anonymous retries. Oyo uses `gh` for authenticated GitHub reads and all GitHub discussion writes, `glab` for GitLab, credentials saved by `cb` for Codeberg, and host credentials saved by `fj` for Forgejo. Oyo does not ask for a separate access token. Private repositories require access through that account.

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

Review URLs stream raw changed lines for GitHub, GitLab, Codeberg and Forgejo, with public access or configured authentication. The first parsed changed file becomes readable while the file list grows. The status says that partial content is unverified and read-only. Comments, reviewed marks and provider writes wait for a validated snapshot.

GitHub reads a fixed-revision comparison through HTTPS or `gh`. Oyo uses `--allow-escape-sequences` when the installed CLI supports it. Older CLIs that stream without that flag retain their API route. Raw provider bytes stay in a private parser pipe. Oyo escapes terminal controls in displayed text.

Configured GitLab uses the merge request raw-diff API through `glab`. Anonymous GitLab uses a fixed-SHA raw comparison. Codeberg and Forgejo use their pull request diff API through the selected account or anonymous HTTPS. These routes do not depend on private web pages accepting API credentials or a server version allowlist.

GitLab, Codeberg and Forgejo validate the raw patch against complete trees at the pinned before and after revisions. Text records must transform verified old bytes into the exact new Git object. This catches a response that ends successfully after omitting a later hunk. Tree enumeration and verification downloads continue after the first readable diff. Large repositories can therefore remain read-only while validation finishes.

Patch context is distinct from complete file contents. Oyo automatically loads the selected file in the background at its fixed revisions. Syntax highlighting appears when those contents arrive. Context stays collapsed. Press `F` to expand all context folds. Use `f` to cycle folding. Selecting another file prioritizes its contents. After the first review frame appears, spare capacity progressively loads the remaining text files. These files retain bytes without computing their full diff or syntax until needed. Verification can also fetch old contents and cache reconstructed new contents. All-file search uses the retained full contents and marks results incomplete while data or snapshot validation is unavailable. It follows the bounded background loading pass and does not restart evicted or failed downloads. A failed download keeps the readable patch and waits for an explicit retry. Refresh the snapshot to retry failed automatic loading. Hydration retains the logical cursor, viewport and saved drafts. An active text selection is cleared with a notice when the layout changes; select the text again before copying or commenting.

Background loading protects the next eight files that fit within 64 MiB and retains a rotating 64 MiB tail. The selected file has a separate 64 MiB allowance. Evicted distant files can require another download when opened. Missing or oversized files are skipped. Access, quota and uncertain transport failures pause further background loading until refresh; selected-file requests remain available. Failed files wait for an explicit retry.

Configured authentication remains selected when credentials fail, access is denied or a quota is exhausted. Oyo does not retry anonymously. GitLab credential environment variables, global or repository-local files, and unknown configuration retain `glab`. A logged-out CLI with an empty configuration can use its anonymous API support. A missing authenticated CLI produces an error. Public reads do not require a CLI when Oyo can establish that credentials are absent.

Commit and comparison URLs retain complete-content loading. Pass `--remote-full` to choose that path for a review, including when a patch exceeds the streaming limits. Its provider enumeration limits still apply.

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

Streaming review manifests come from raw patches, without a capped changed-files listing. GitHub checks exact file and addition/deletion counts. GitLab, Codeberg and Forgejo require complete immutable tree coverage and verify each record's paths, modes and object identities. Capped GitLab counts such as `1000+` do not block this tree-verified path. All providers require successful transport completion, strict parsing and a final repository and review identity check.

Metadata-only changes, empty files, renames, binary records, symlinks and mode changes remain manifest entries. Binary records verify object identities and modes; Oyo does not reconstruct binary patch payloads for display. Non-UTF-8 text retains the binary display state, while its text patch still receives exact byte verification. Submodules, copied patch records, split deletion/addition type replacements at one path, and non-SHA-1 object formats are unsupported.

GitHub complete-content review enumeration rejects lists beyond 3,000 files. GitHub commit enumeration stops at 3,000 files, and comparisons stop at 300 files. GitLab complete-content review loading requires an exact changed-file count. Oyo rejects potentially truncated results.

A streamed patch is limited to 48 MiB in total, 32 MiB per file record or line, 250,000 lines per file record and 100,000 files. Malformed, combined, truncated and over-limit patches produce an explicit incomplete state. Git paths containing control characters, backslashes or traversal components are rejected. Use `--remote-full` when a supported complete-content path can handle a patch outside these bounds.

Complete-content review enumeration stops after 100 pages. Streaming tree verification allows 100,000 entries per side and 1,000 populated pages, followed by a terminal check where required. Other tree enumeration stops after 1,000 pages. Forgejo merge-base searches stop after 2,048 commits and reject missing or multiple merge bases. Use a two-dot comparison when you need a direct endpoint comparison.

Sparse-review search examines at most 32 MiB across 1,024 retained files, with a 200,000-line old-and-new input budget. Files outside those budgets are reported as not searched. This bounds search requests, corpus construction and retained results while other files continue loading.

Sparse review sessions retain at most 192 MiB of core file text, with at most 1,024 files in the rotating tail. This is not a process memory limit: transport, verification, byte caches, tree pages, active requests and the selected display use additional memory. Shared source and tree caches each retain up to 64 MiB of evictable completed payloads and 128 entries; active producers and borrowed payloads remain separately bounded. The verified byte cache retains up to 64 MiB and 1,024 entries. Commit, comparison and --remote-full sessions retain their existing complete-content loading behavior.

Individual file downloads are limited to 32 MiB. Each transport request has a 30-second deadline. Binary and oversized files display explicit states. Saved full-file comment captures retain the 256 KiB per-side limit; larger files may lack a full outdated captured view.
