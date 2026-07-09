# Backlog

Prioritized. P0 = correctness / data integrity, P1 = bugs + the core review loop,
P2 = docs and cleanup, P3 = nice-to-have.

## P0 — Bug: reviews keyed by diff_fingerprint desync (comments lost/duplicated)

Status: FIXED & VERIFIED (core). Reviews are now stored by a stable `reviewKey`
(`jj:change:<ws>:<change_id>`, `git:worktree:<ws>`, and scoped keys for
staged/file/branch/target) instead of the moving diff fingerprint. Verified:
`status` shows the real comments (was 0), `add` reflects, `rm` truly deletes
(displayed == mutable), and `rm` on a bad id errors "No comment matches id N" (no
false success). Cleanup DONE: (a) repair-path removal was a no-op — no one-shot
repair/dedupe/migration code exists; all DB access is scoped to a single stable
key (`WHERE diff_fingerprint = ?1` bound to the reviewKey), no cross-fingerprint
scan. (b) Dev DBs cleaned surgically — the two bloated DBs (`3f4d08d2` = this
repo's live jj review, `c7435644` = a real branch review) held live stable-key
comments comingled with orphan bare-hex fingerprint churn, so a blanket file wipe
would have destroyed live data; instead deleted only the orphan rows
(`diff_fingerprint NOT LIKE '%:%'`): 84 rows removed (73→14, 32→7), all live
comments preserved, backups in scratchpad. Column rename DONE & VERIFIED: the SQL key column `diff_fingerprint`
(which held the reviewKey) is renamed to `review_key` across the `reviews` and
`comments` DDL, PK, and every query (incl. the main.rs review-log join). No
migration code in oyo — the 16 existing local dev DBs were patched externally with
in-place `ALTER TABLE … RENAME COLUMN` (backed up first); verified the new binary
loads the oyo repo review (15) and the dogfood PR #1 (7), and fresh DBs are born
with `review_key` + `PRIMARY KEY (review_key, id)`. The real diff-fingerprint
concept (`review_diff_fingerprint`/`OYO_DIFF_FINGERPRINT`/`diffFingerprint` JSON) is
untouched.

Reviews are keyed by `diff_fingerprint`. Under jj, `@` is amended constantly, so
the fingerprint moves and the review layer diverges badly:
- The DB accumulates a **new fingerprint copy of every comment on each amend**.
  Observed: **24 fingerprints / 68 comment rows for ~6 real comments.**
- `oy review status` / `comment` (CLI) resolve `@` to the latest computed
  fingerprint, which may hold **0** comments while the user's real comments sit
  under a slightly older fingerprint. Observed: CLI showed `count 0` (fp
  `9e615e72`) while 6 user comments existed under fp `1ce77503`. An agent using
  `oy review comment` as its task list would get **nothing**.
- `rm`/`edit`/`abandon` act on the current fingerprint and cannot touch the
  stranded copies — yet `rm` still prints "Removed comment N" (false success).

Fixes:
1. Key the logical review by **change_id** (jj) / stable branch or commit (git),
   not the moving diff fingerprint — so amends do not strand or duplicate.
2. Make CLI `status`/`comment` and the TUI resolve `@` to the **same** review, and
   the DISPLAYED set == the MUTABLE set (`rm`/`edit` can delete whatever is shown).
3. `rm`/`edit` must **error** ("No comment matches id N") when nothing is removed —
   never false success. *(partial: shared resolver now errors on no-match)*
4. The DEFAULT git review (bare `oy`, uncommitted working tree, no target) has the
   SAME churn — the diff fingerprint changes as you edit files — so it also needs a
   stable key (e.g. the worktree root), not just branches/commits/ranges.
5. Do NOT write migration/repair code — unshipped feature. Remove the one-shot
   repair path and wipe the dev review DBs instead of migrating them.

Follow-on — anchor drift: once comments persist across amends/edits under a stable
key, their `file:line` anchors may no longer match the evolved diff. Decide how
stale anchors behave — re-anchor to the new diff, or mark them **outdated** like
GitHub. (Deliberate call; can be scheduled after the key fix.)

## P0 — Bug: concurrent local writes lose comments (whole-snapshot last-writer-wins)

Status: DONE & VERIFIED. Persistence no longer delete-all/reinserts a stale whole
snapshot — it opens an IMMEDIATE txn (busy timeout), upserts only changed comments
via INSERT ON CONFLICT, deletes only baseline rows actually missing, and reassigns
a new comment's id under the write lock if it collides with a concurrently
committed id. Regression tests added. Independently verified from the CLI with real
concurrent processes: (a) two concurrent adds on different lines → both survive
(ids 1,2); (b) same-id collision path (both writers from an empty review) → 6/6
rounds, 0 losses, colliding id reassigned to [1,2]; (c) concurrent edit(#1) +
add(#3) → edit sticks, add present, #2 untouched, count 3.

Data integrity; was a latent multi-agent bug.
Spec: /tmp/oyo-concurrent-write-loss-from-claude.md. Independent of the id
scheme — a uuid would NOT fix this; the durable provider id is already handled via
`ReviewComment.provider.comment_id`. This is purely the LOCAL persistence path.

Local review persistence is a read-modify-write of the WHOLE comment set: every
`oy review comment new` (and edit/resolve/rm) loads the entire review into memory,
mutates it, then in one transaction does `DELETE FROM comments WHERE review_key`
followed by re-INSERT of the full in-memory set (`review.rs:4720` + the insert
loop after). It is a last-writer-wins snapshot, not a row-level merge.

So two writers on the SAME local review DB concurrently (the whole premise of oyo
— e.g. oy-gpt in the TUI + Claude via the CLI, or two CLI invocations) race: the
second to commit never saw the first's new comment, and its snapshot DELETE+INSERT
silently overwrites it. One agent's comment is lost with no error. This is a live
gap given multi-agent is the core use case.

Note: this is the LOCAL DB only. Push/pull to a provider is a separate path and
carries its own durable id (`provider.comment_id`), so this does not affect synced
comments — only concurrent local mutation.

Fixes (pick one, smallest first):
1. Row-level writes: `INSERT ... ON CONFLICT(review_key,id) DO UPDATE` per changed
   comment + targeted `DELETE` for tombstones, instead of delete-all-then-reinsert.
   Then two writers touching DIFFERENT comments no longer clobber each other.
2. Optimistic concurrency: a per-review version/rev (we already have
   `review_revision`); re-read + bump under the write lock, abort/retry if the
   on-disk rev moved since load, so a stale snapshot cannot overwrite newer rows.
3. At minimum, hold a SQLite IMMEDIATE/write transaction across the read→mutate→
   write so the allocation + snapshot are serialized, not interleaved.
Repro: run two `oy review comment new` against the same review near-simultaneously
(or one in the TUI while another via CLI); one comment goes missing from the DB.

## P1 — Bug: PR add-comment affordance missing when a PR exists but has no comments

Status: DONE & code/test-VERIFIED (live-PR end-to-end pending user in TUI). Fix:
plain branch/worktree reviews now start a background PR lookup
(`spawn_review_pr_lookup_worker`, main.rs:3794, reuses gh_pr) when no explicit PR
target or existing PR comment identifies one; success caches
provider/remote/repo/number/title as `review_pull_request_target` on App;
`pull_request_comment_target_available` (review.rs:2254) + `pull_request_title`
consult the cache. Worker polled via `try_recv()` in the main loop (non-blocking,
never in render). Missing gh / offline / no-PR stay silent ("No pull request
found."). Verified: both render tests pass (`discovered_pr_without_comments_shows_
add_comment`, `pr_comment_view_without_pr_hides_add_comment`), cache-consult +
non-blocking poll confirmed in code. Live GitHub end-to-end (real empty PR ->
button) best confirmed by Victor in the TUI.

Spec:
/tmp/oyo-pr-add-comment-no-comments-from-claude.md. Reported by Victor from the
TUI. On a branch with a real open PR but zero PR comments (plain branch review, not
PR-targeted), the PR-comments view shows "No pull request found." and no
add-comment button. Root cause (code-confirmed): the affordance is gated on
`pull_request_comment_target_available()` (review.rs:2215), which returns true only
if `metadata.pr_number` is set (explicit PR target only) OR a PR comment already
exists — it never checks whether a PR actually EXISTS for the branch, though
`gh_pr` (main.rs:3141, `gh pr view`) can discover it. Fix: cache a background PR
lookup on load/target-change/refresh (NO synchronous gh in the render path), have
`pull_request_comment_target_available()` consult it, show "No pull request
comments." + the add affordance when a PR exists with no comments, keep "No pull
request found." only when there truly is no PR. Verified via code, not a live PR.

## P1 — Anchor drift: persisted comments go stale as the diff evolves

Status: DONE & VERIFIED (all 3 phases). Phase 1: line & hunk anchors
now capture an additive `anchorSnapshot` at creation — `lineText`,
`contextBefore`/`contextAfter` (±3, clamped), `side`, `lineNumber`, and target
(jj change/commit or git base/head commit) — stored in the blob, exposed as
`anchorSnapshot` in review JSON, no migration. Verified from the CLI: comment on
an edited line captured `lineText` (actual content, not a hash), context correctly
clamped, and the git base commit. Phase 2 DONE & VERIFIED: load/reload
reconciles snapshot anchors against current same-file/same-side content — exact
stays, shift updates ranges/anchor_key/hint/hunk_id + `reanchored=true`, missing
text -> `outdated=true` (kept, not deleted/resolved, excluded from live overlays),
and an outdated comment re-checks and clears if the text returns; `outdated` +
`reanchored` exposed in JSON. Verified with real git repros: insert-3-above -> L3
re-anchors to L6 (reanchored=true, outdated=false); edit-the-line -> outdated=true
(snapshot kept); restore -> outdated clears; reload twice -> idempotent.
Phase 3 DONE & VERIFIED: human block shows `Status: unresolved (outdated)` (color
on TTY only, `2;33` dim-yellow tag); `--outdated`/`--no-outdated` on comment +
status; `--unresolved` excludes outdated by default, `--unresolved --outdated` =
intersection; `status` commentCount follows the filtered rows; `--json`/pipe stay
clean. TUI dims resolved/outdated sidebar+picker entries and tags outdated; live
inline overlays still exclude outdated. Verified from the CLI with a live+outdated
mix: all filters, the count, and pipe/json cleanliness pass.

Spec: /tmp/oyo-anchor-drift-from-claude.md.
The deliberate follow-on to the stable-reviewKey P0. A line anchor is
COORDINATE-ONLY (`anchor_key = "line|<file>|<side>|<line_no>"`, review.rs:933) —
no stored content, no target sha — and matching is line-number equality. Now that
comments persist across amends, an insert above a comment leaves it on the WRONG
code (line shifted), or an edit/delete orphans it, and oyo can't tell which
because the anchor carries no content.

Behavior: re-anchor first (silent) when code merely MOVED; mark OUTDATED
(GitHub-style, keep original snippet, collapsed/dimmed) when the commented code
changed/vanished. Never auto-delete or auto-resolve; outdated != resolved.

Plan (advisory): Phase 1 (prereq, ships alone) — capture the anchored line TEXT +
context (+/-3) + target sha/change_id at comment time; store line-text NOT a bare
hash (can't relocate from a hash); additive to the opaque JSON blob, no migration.
Phase 2 — reconcile in the load_review_state / repair_review_comment_file_indexes
path: exact (content matches) -> keep; captured text found elsewhere -> shift/
re-anchor; not found -> outdated=true + keep snippet + drop from live set. Phase 3
— surface it: TUI dimmed "Outdated" card (reuse resolved treatment); CLI
`Status: outdated` + `--outdated` filter, exclude outdated from `--unresolved`
(ties into `--since`). First cut = exact/shift/outdated via simple text matching;
fuzzy/rename-follow is a later refinement.

## P1 — Bug: split-view review-card avatar mispositioned, floats on scroll

Status: FIXED & VERIFIED (user-confirmed in the TUI). oy-gpt's fix: split panes
used the global scroll offset for avatar viewport math even after skipping rows to
local indices; now `viewport_start = 0` for non-wrapped split, matching the
review-card hitbox logic (scroll_offset kept for wrapped).

In split view the review comment card's author avatar drifts away from its card
while scrolling (unified is fine). Likely the avatar's screen position uses the
wrong coordinate space for the split layout (unified offsets, or ignoring the
split point + scroll offset). See `crates/oyo/src/avatars.rs` + the split
review-card render in `crates/oyo/src/ui.rs`.

## P1 — Review comments: resolve / unresolve (GitHub-style)

Status: FIXED & VERIFIED (CLI + TUI, user-confirmed). CLI: resolve/reopen flip the
`resolved` field, `--unresolved` filters both the list AND `commentCount`
(regression test added). TUI: `r<card>` toggle next to edit/delete, footer shows
resolve/reopen, resolved cards get a check-mark, PR conversation cards excluded.

Add a resolved state (default **unresolved**), like GitHub's "Resolve
conversation", so "treat unresolved comments as the task list" is real: the human
opens threads, the agent works the unresolved set and resolves each. There is no
resolved concept anywhere today.
- **Data**: `resolved: bool` (default false). Cheap — comment is an opaque JSON
  blob, no migration.
- **TUI**: a **toggle** button on the review card, right next to `ia edit` /
  `xa delete`, with a per-card hotkey in the same scheme — e.g. `ra` / `rb` …
  (mirrors `ia`/`xa`; `r` = resolve, mnemonic — quick collision check vs global
  `R` refresh, but card hotkeys are contextual). It **toggles**: `r<card>`
  resolves an open comment and **reopens (unresolves)** a resolved one; the label
  + visual flip accordingly (e.g. "resolve" when open, "reopen" / a check when
  resolved). This is the half a UI-only user needs to close threads.
- **CLI**: `oy review comment resolve <id>` / `reopen <id>` + `--unresolved` filter.
- Scope: inline comments (line/hunk/file). **PR conversation comments
  (`kind:"pr"`) are excluded** — same as GitHub — so the **PR comment preview card
  shows NO resolve button.**

## P1 — Comment change detection: timestamps + since/version

Status: FIXED & VERIFIED (CLI). `createdAt`/`updatedAt` set on create and bump on
edit/resolve/reopen/delete (monotonic even within a second); `--since <ts>` on
`comment`/`status` returns per-comment `changeType` (added/updated/removed) with
hidden tombstones so deletions are reported; regression tests added. CLI-only, no
visual component.

Local comments come back with `created_at: null` / `updated_at: null` (only pulled
provider comments carry timestamps). So an agent can spot adds/removes by `id` but
cannot detect **edits** except by diffing bodies, and there is no "what changed
since" query.
- Stamp `created_at` on create and bump `updated_at` on every edit/resolve.
- Add `--since <ts>` or a monotonic per-review version/seq (review-side analogue
  of control's `lastAppliedSeq`) so agents can poll efficiently.

## P2 — Split docs/CONTROL.md into a CLI reference (skill stays separate)

Status: DONE & VERIFIED. `docs/CONTROL.md` rewritten as an `oy control` CLI
reference (no frontmatter, opens `# Control commands`, human register); confirmed it
differs from the skill copy and `oy skill path control` still installs the skill
with frontmatter. Comment #17 resolved. Spec:
/tmp/oyo-control-md-cli-doc-from-claude.md. From Victor's comment #17. Two
identical CONTROL.md copies: `crates/oyo/docs/CONTROL.md` = the skill
(`include_str!`, installed as oyo-tui-control) — correct; `docs/CONTROL.md` (repo
root) = a copy of the same skill prose, misfiled among human docs, so `oy control`
has no human CLI reference (unlike `oy review`/`docs/REVIEW.md`). Fix: rewrite
`docs/CONTROL.md` ONLY as an `oy control` CLI reference mirroring `docs/REVIEW.md`
(no frontmatter, `# Control commands`, capability sections, drop the do-not-fake-
keys agent intro); leave `crates/oyo/docs/CONTROL.md` as the skill (they
intentionally diverge). Optional: rename the skill source to end the name
collision. Verify: docs/CONTROL.md no frontmatter + mirrors REVIEW.md + differs
from the skill copy + skill still installs.

## P2 — Rewrite CONTROL.md as a manual + add an installed control skill

Status: DONE & VERIFIED. docs/CONTROL.md rewritten as a manual (frontmatter,
Workflow, session selection, grouped Commands, special cases, guiding review,
Common errors); installed as the `oyo-tui-control` skill via `oy skill path
control`; review skill cross-refs it. Bonus: fixed the goto `--step` conflict
(renamed to `--step-number`).

Docs an agent loads or a user follows should be operational manuals — imperative,
task-first, copy-pasteable — not design rationale. `docs/CONTROL.md` is still
headed "Status: design proposal and implementation guide"; once `control` ships,
rewrite it into a how-to. Also add a proper **installed control skill** an agent
can load (like hunk has one), not just prose.

Template (adopt the STRUCTURE, in oyo's OWN voice — do NOT copy hunk's text):
the `hunk-review` SKILL.md
(`~/.npm-packages/lib/node_modules/hunkdiff/skills/hunk-review/SKILL.md`) is the
model. Sections: frontmatter (`name` + when-to-use `description`); one-line
boundary intro (TUI is the user's — steer via the CLI, do not fake keys); a
numbered **Workflow** happy path; **Session selection**; **Commands** grouped
(Inspect / Navigate / Target / Modes / Comments) each a code block + tight bullet
notes; special cases; a **Guiding a review** narration section; and a **Common
errors** list mapping each error string to the fix. GOV.UK style throughout.
oyo's commands map ~1:1 (list/get/where/diff/goto/target + oy review comment).

## P2 — `-s` shorthand for `--session` on control subcommands

Status: DONE & VERIFIED. `-s` is now the `--session` short form on `oy control`
and its subcommands; `oy control where -s <name>` parses and help shows
`-s, --session`. Top-level `-s`/`--speed` confirmed unchanged. Session selection
docs (`docs/CONTROL.md`) + the installed `oyo-tui-control` skill both show
`--session <name>` or `-s <name>`; old "do not add `-s`" warning removed.

## P2 — Drop agent shorthand if the env vars cover it

Status: DONE & VERIFIED. Shorthand removed (now rejected as an unexpected arg);
verified the `OYO_REVIEW_AUTHOR_*` env path attributes agent comments and the
explicit `--author-*` flags still work; docs updated. Kept the env path and the
`--author-*` flags.

With `OYO_REVIEW_AUTHOR_TYPE=agent` set for the session, every comment is already
agent-attributed, so the extra shorthand was redundant.

## P2 — Skill should reference the live TUI control (CONTROL.md)

Status: DONE & VERIFIED. SKILL.md has a "Control a running TUI" section pointing to
docs/CONTROL.md with starter commands (`oy control list`, `oy control where --json`)
and the control/review boundary, written in GOV.UK style. Separate installed
control skill deferred until a control-only entry point is wanted (`oy skill path`
is singular).

`crates/oyo/docs/SKILL.md` does not point at `docs/CONTROL.md` (`oy control`), an
agent capability. Cross-reference it, and consider promoting CONTROL.md to its own
installed skill discoverable via `oy skill path`.

## P2 — `--id` filter on `oy review comment` (read one comment by id)

Status: DONE & VERIFIED. `--id <n>` is a repeatable filter on `oy review comment`
AND `oy review status`; composes with targets + other filters, works with
`--json` (one-element array), and errors non-zero "No comment matches id N." on a
miss (regression tests added; REVIEW.md + review skill updated). Verified from the
CLI: `--id 14` reads one, `--id 9999` errors exit 1, `--id 14 --json` one-element,
`--id 7 --id 14` returns both. Read counterpart to `edit`/`rm`/`resolve
<id>`. Deliberately NOT a `view` verb (the bare command is already the reader;
filters narrow it, verbs mutate) and NOT a target positional (a comment id is
scoped within a review — target selects the review, id selects the comment inside
it, so putting an id in the target slot is ambiguous). Instead `--id <n>` joins
the existing filter family (`--unresolved`, `--author`, `--author-type`,
`--since`). Must error "No comment matches id N" (non-zero, no false success),
work with `--json` (one-element array), and compose with `[TARGET]`. Enables the
copy-id round-trip: the id copied from a listing feeds `--id`/`edit`/`rm`/`resolve`
identically. Spec: /tmp/oyo-review-comment-id-filter-from-claude.md.

## P1 — Bug: review skill links a `./REVIEW.md` that never ships

Status: DONE & VERIFIED. Review SKILL.md is now self-contained: dead
`[Review commands](./REVIEW.md)` link removed (repo + installed), the command
reference is folded inline (Workflow, Choose the target, Show reviews, Read/Work
on/Add/Update comments, PR pull-push, Export/apply, Abandon, Common errors), and
control is still cross-referenced by runtime command (`oy skill path control`, not
a sibling link). Verified: `grep REVIEW.md crates/oyo/docs/SKILL.md` empty,
installed skill carries the reference inline, comments #14/#15 resolved by oy-gpt.

The review skill (`crates/oyo/docs/SKILL.md`
line 12) linked `[Review commands](./REVIEW.md)`, which was dead two ways: (1) in the
repo it resolves to `crates/oyo/docs/REVIEW.md`, which does not exist — REVIEW.md
lives at repo-root `docs/REVIEW.md`; (2) install only writes `SKILL.md` per skill
dir (`include_str!("../docs/SKILL.md")`, `main.rs:71`), so REVIEW.md is never baked
or installed — `find ~/.local/share/oyo -name REVIEW.md` returns nothing. So the
end user's agent hits a dangling link. Contrast: CONTROL.md IS baked
(`include_str!("../docs/CONTROL.md")`, `main.rs:72`) and installed as
`oyo-tui-control/SKILL.md`, byte-identical, reachable via `oy skill path control`.
REVIEW.md is the only one of the three skill docs that does not ship.

Fix (approach A — self-contained, mirrors control): fold REVIEW.md's command
reference INTO the review `SKILL.md` and drop the `./REVIEW.md` link entirely, so
the review skill is self-contained just like the control skill is (its whole
manual IS the SKILL.md). Rule to hold: only `SKILL.md` ships, so a skill must be
self-contained or reference by runtime command — never a relative path to a
sibling that does not travel with it. (#15 note: the review skill DOES already
cross-ref control via `oy skill path control`; a separate consolidating/index
skill is optional and only worth it once there are more than two skills.)

## P2 — Color the `oy review comment` block output (match `oy review status`)

Status: DONE & VERIFIED. `format_review_comment` now paints via the shared
`review_cli_paint`/`review_cli_color_enabled` path; regression test added.
Verified from the CLI (pty): id `1;38;5;8`, status semantic (unresolved `33`
yellow / resolved `32` green), File+Location `2`, labels `2`, agent author tag
`35` magenta, body plain; piped / `NO_COLOR=1` / `--json` all emit zero ANSI.

`oy review status` is fully painted
(`main.rs:2367-2380`: id bold `1;38;5;8`, subject cyan `36`, location dim `2`,
"local changes" `33`) but `format_review_comment` (the `oy review comment` block)
has ZERO `review_cli_paint` calls — same tool, two looks. Paint the block with the
EXISTING infra (`review_cli_paint` + `review_cli_color_enabled()`, which already
respects NO_COLOR / non-TTY; agents read `--json` so machine output is untouched).
Mapping, semantic not decorative: `#id` bold same code as status; `Status:` value
SEMANTIC (unresolved `33`, resolved `32`, removed `2`); `File:`/`Location:` dim
`2`; labels (`Author:`/`Body:`) dim `2`; author type `(agent)` magenta `35` like
status' "from"; body text stays default for readability. One accent + semantic
status + dim labels — do not rainbow it. Reuse status' exact codes so the two
commands look like one tool (the consistency is the real win).

## P2 — Replace `=== Comment #N ===` banner with an `ID:` field

Status: DONE & VERIFIED. Blocks now lead with `ID: #N` (bold `1;38;5;8`), `===`
banner removed entirely, tests updated. Verified: piped output starts `ID: #7`,
zero banner lines, TTY keeps id-bold + status/label/author-type colors, `--json`
no ANSI.

The `=== Comment #N ===` header in
`format_review_comment` was the only line not in the `Label: value` shape every
other field uses — it reads as decoration. Replace it with `ID: #N` as the FIRST
(lead) field, uniform with `File:`/`Location:`/`Status:`/etc. Keep the `#` sigil
(`ID: #14`, not `ID: 14`) for parity with `oy review status` and edit/rm/--id;
keep the bold id color (`1;38;5;8`) on the value. Rely on the existing blank line
between records + the bold lead id for separation (the banner's real job was
delimiting a long list, so `ID:` must stay first, not buried mid-block). Follow-up
on the color ticket — same `format_review_comment` fn.

## P2 — `/` search should be a floating find bar (match the sidebar filter)

Status: DONE & VERIFIED (code/test; visual pending user in TUI). `draw_find_bar`
(ui.rs:769) + `find_bar_area` (735) render a rounded float with `❯` prompt, query,
blinking `│` (`search_cursor_visible`), `current/total` count, `‹`/`›` chevrons and
`✕` clear — all click hitboxes (`search_prev_hit`/`search_next_hit`/
`search_clear_hit`) with hover; corner-avoidance scans the active highlight each
draw (top-right → top-left → bottom-right → bottom-left); search removed from
`line_input_status_spans` (goto stays in the status bar); narrow viewports clamp.
Verified: the render test asserts rounded corners, prompt/query/`1/2`/`│`/`‹`/`›`/
`✕`, hitbox hover+click (next advances the match), corner movement, and clear.
Follow-ups (verified): cursor now follows the query (padding moved after cursor,
asserts `❯ target│`); `✕` now CLOSES the bar (not just clears text), its hitbox
includes the trailing cell for glyph-width, and the whole float consumes clicks so
the underlying add-comment affordance can't receive them.

Spec:
/tmp/oyo-find-bar-from-claude.md. Pressing `/` runs search but renders the input
in the CENTER of the status bar (ui.rs:1211-1234 build the `/`+query+"Search"
prompt; `line_input_status_spans` feeds the center section at ui.rs:1328) —
cramped, shares space with `:` goto and the step counter. Bad UX.

Fix: a proper FLOATING find bar, curved (rounded) border, styled to MATCH the
sidebar's `/` filter (`file_filter_line`, ui.rs:3261). Elements left→right:
`❯ ` prompt (theme.primary bold) · query (theme.text) · blinking `│` cursor (add a
search-side blink state like `file_filter_cursor_visible`) · match count
`current/total` (e.g. `2/7`) · prev chevron `‹` · next chevron `›` · clear `✕`.
Float mechanics reuse `draw_file_search_popover` (ui.rs:654 — `Clear` +
rounded-border `Block` + theme bg + border_active). Each chevron and the `✕` are
click hitboxes (like `file_filter_clear_hit`), mirrored by `n`/`N` keys.

Default position TOP-RIGHT of the diff view; CORNER-AVOIDANCE: if the current
match's cell overlaps the bar rect, hop corners in order top-right → top-left →
bottom-right → bottom-left, landing on the first non-overlapping one; re-evaluated
on each `n`/`N` jump so the bar keeps dodging the highlighted hit.

Notes:
- `:` goto shares the same center-bar prompt (`line_input_status_spans` handles
  both). Consider unifying goto into the same float for consistency — oy-gpt's call.
- Keep existing search behavior (match nav, highlight); count/chevrons/avoidance
  are new affordances on top.

## P2 — Keyboard hotkeys to cycle focus between review cards in the diff

Status: DONE & VERIFIED. `NormalAction::FocusNextComment`/`FocusPrevComment`
(keybindings.rs:114) with defaults `}` / `{`; `focus_next/prev_review_comment`
(review.rs:1927) cycle all non-deleted comments in document order, reuse
`open_review_comment` (file select/scroll/PR/flash) in unified+split. Verified: the
regression test cycles `10→20→30` from scrambled input, wraps both ways, from
no-focus prev→last, includes resolved+outdated, excludes deleted; 2 tests pass.

Was: there is no keyboard way to move
focus from one review comment card to another inline in the diff. Today `active_review_comment_id` is set
only by clicking a card / creating / editing (review.rs:1935 etc.); the only
keyboard paths are the comment picker overlay (`open_comment_picker`, ctrl-shift-c)
and scrolling the comments sidebar — neither is a quick "next card / prev card"
cycle in the diff.

Fix: add `focus next / prev review comment` actions that set
`active_review_comment_id` to the next/prev comment in DOCUMENT order (the same
file/line sort as the listing), scroll it into view (reuse the anchor scroll path,
`review_anchor_display_span` review.rs:2831), and CYCLE (next past the last wraps
to the first; prev before the first wraps to the last). Works in unified AND split.
Config-driven keybindings (e.g. `normal.focus_next_comment` /
`focus_prev_comment`); oy-gpt picks non-colliding defaults (bracket keys `[`/`]` or
Tab/Shift-Tab are candidates). Open question for oy-gpt: cycle ALL non-deleted
comments, or skip resolved/outdated — default to all; a filtered cycle can follow.

## P1 — Bug: resolving a comment via CLI makes it vanish from the diff

Status: DONE. Missing file-index data no longer marks a comment outdated. Anchor
reconciliation now changes `outdated` only when loaded content fails snapshot
matching. A regression test covers a resolved comment with no diff-map match.
CLI-verified live: `oy review comment resolve 22` → resolved=true, outdated=false
(was the bug). Spec: /tmp/oyo-live-reload-bugs-from-claude.md.

## P1 — Bug: live reload drops hunk colouring and decorations until restart

Status: DONE. File reload now invalidates derived view, hunk and render caches,
rebuilds the visible file's syntax cache and warms lazy checkpoints before the next
draw. Watch tests cover direct file refresh and a Git file-list refresh. Visual TUI
confirmation remains. Spec: /tmp/oyo-live-reload-bugs-from-claude.md.

## P2 — GitHub-style expandable context folds (2 phases)

Phase 1: DONE AND VERIFIED. Spec: /tmp/oyo-fold-phase1-from-claude.md.
Expandable folds keep edge context, reveal 20 lines from either side, preserve
step targets and work in unified and split views, including wrapped lines. Search
skips fold labels. Large-file windowing is disabled while folding is on to avoid
double collapse.

Phase 2: DONE. Spec: /tmp/oyo-fold-phase2-from-claude.md. `FoldContextMode` now
contains only Off and Expandable, with Expandable as the default. `f` toggles the
two states. `F` expands all folds. `fold_context = "off"` keeps full context by
default, and `fold_context_lines = 0` gives maximum expandable compaction. The
8-line hidden-context threshold remains. Visible inline comments act as virtual
hunks, keeping their line and edge context open. Resolved comments anchor folds;
outdated, deleted and pull request comments do not.

Fold-band tuning: glyph, "unchanged line(s)" wording and the theme-derived dim
row background are done. Scope hints remain a follow-up. Oyo computes diffs from
file contents and does not ingest Git's unified-diff section heading, so adding a
correct enclosing scope needs new diff metadata or language-aware scope detection.
Do not use a declaration-name heuristic. Spec:
/tmp/oyo-fold-band-tuning-from-claude.md.

Fold hotkeys: DONE. The global expand-nearest action and `O` binding are removed.
Each visible fold uses contextual top and bottom shortcuts generated through
`review_index_action_label`, following the review-card action pattern. Shortcut
labels and arrows use the muted text colour by default; hovering a direction
switches its shortcut and arrow to bold accent together. Fold affordance rows are
not selectable and are excluded from copied selection text. `F` remains the
only global expansion shortcut.

Fold-band refinements: DONE & VERIFIED. (1) `ua`/`da` key codes accent-coloured;
initially always-bold to match the card, then a follow-up made them UNBOLD by
default and BOLD on hover (per direction pair; hovering the key or its arrow bolds
the pair, hitbox covers key+arrow). NOTE divergence: review-card hotkeys are
always-bold; fold hotkeys are now bold-on-hover only — deliberate (calmer across
many bands); revert to always-bold is a one-liner if exact card-match is wanted. (2) fold rows are fully NON-SELECTABLE —
tracked as `fold_context_screen_rows` (selection.rs:792, populated from
`is_fold_line` during unified and split render): mouse start rejected,
drag/keyboard extension jumps over, highlight skips, action metadata omits, copied
text has only real code lines. 11 fold + 15 selection tests pass (new unified/split
coverage for anchoring, drag, keyboard, highlight, exact copied text).

Fold-band scope hint: DONE. Syntect finds and caches the innermost named definition
for each fold. Unified and split bands show its definition line dimmed. Narrow
bands truncate the line before compacting controls. Top-level code, plain text,
unsupported languages and files over 512 KiB show no hint. Verified (code/test;
visual pending user): scope engine, per-file cache and dimmed render confirmed.
Tests cover nested Rust, JavaScript and Python scopes, impls, call rejection, empty
cases, mixed tabs, cache reuse, width clamping, dim styling and unified and split
views. Spec:
/tmp/oyo-fold-scope-hint-from-claude.md.
Follow-up: DONE. Bands now show the leading-whitespace-trimmed definition line,
including the trailing `{`. Long declarations end with an ellipsis and fit the
remaining width. Controls still win. Innermost selection, caching and empty cases
are unchanged. Spec: /tmp/oyo-fold-scope-line-from-claude.md.
Markdown follow-up: DONE. Markdown headings own their section body until the next
heading of the same or higher level. Nested sections use the innermost heading,
preamble stays empty and code parsing is unchanged. Spec:
/tmp/oyo-fold-scope-markdown-from-claude.md.

## P1 — Bug: folding hides a comment on a context line (lost-comment footgun)

Status: DONE & VERIFIED (code/test; visual pending user). `fold_context_view` now
takes `comment_anchors: &FxHashSet<usize>` and excludes those change_ids from
collapsible runs (utils.rs:323/332); the anchor set (`review_fold_anchor_change_ids`,
review.rs:3314) reuses the SHARED `review_comment_is_inline_visible` predicate + Line
kind, so resolved anchors / deleted+outdated+PR don't (fold and overlay can't
disagree). Anchor-set hash in the view cache key rebuilds folds on add/delete/
outdated/reload without editor churn. Tests: `comment_anchors_split_folds_and_keep_
edge_context`, `visible_context_comment_anchors_folds_but_outdated_comment_does_not`;
11 fold tests pass (282 total).

Spec:
/tmp/oyo-fold-comment-anchor-from-claude.md. Reported by Victor. Comments can
anchor to context/unchanged lines (review.rs:1165), but `fold_context_view`
(utils.rs:20/35) collapses context runs without checking for comments — so
expand→comment-on-context-line→re-fold HIDES the comment + card (still in data,
invisible in view; same family as resolve→outdated). Fix: a commented line is a
fold ANCHOR treated like a hunk — extend is-anchor from "has a change" to "has a
change OR carries a visible comment"; it becomes a virtual hunk (kept + edge
context `fold_context_lines` each side, surrounding gap splits into two expandable
folds), so the comment shows IN CONTEXT. Nuance: anchor ONLY if the comment renders
inline — reuse the overlay filter (review.rs:3287, not deleted/outdated/PR), so
resolved comments anchor (stay visible) but outdated ones don't (already hidden,
must not force-unfold). Only changes WHICH lines are foldable; all other fold
behavior unchanged.

## P2 — Bug: picker cursor sits after the placeholder when empty

Status: DONE. `picker_input_line` places the cursor before an empty placeholder and
after a non-empty query. Command, file, comment and theme pickers share the fix. A
regression test covers visible, typed and blink-hidden states.

## P2 — Search-match highlight contrast (readable fg on every match)

Status: DONE & VERIFIED (code/test). `/` matches sometimes had poor fg/bg contrast:
`highlight_search_spans` (search.rs:317) only gave the ACTIVE match a contrast-aware
fg (others kept inherited syntax fg on the dim-accent bg), and `search_highlight_fg`
only picked best-of {text, background} with no floor. Fix: contrast-aware fg for
EVERY match against its own bg; if best-of-{text,background} is below WCAG 4.5:1,
fall back to pure black/white (whichever contrasts more). Accent/dim-accent bg
unchanged so active vs inactive stay distinct. Regression test with clashing
theme+syntax colors covers both states + the 4.5 floor; test passes. Spec:
/tmp/oyo-search-contrast-from-claude.md.

## P1 — Bug/feature: clicking an outdated comment strands you; add an Outdated tab

Status: DONE & VERIFIED (code/test; TUI visual pending user). `TopbarTabContent::
OutdatedComments` wired through all match arms + `outdated_comment_focus` routing;
`g o` + palette + control open it; sidebar/picker route outdated selections there
(dead-end fixed); cards show author + Outdated tag + original file:line + snapshot
(anchor line marked) + edit/resolve/delete. PR/MR nouns: `short_review_noun`/
`long_review_noun` on ReviewProviderKind (GitLab→MR/merge request, else PR), test
asserts it, drives tab label/palette/title/empty copy. 8 outdated tests pass (323
total). Installed skill now has `g o` (=3) AND `--outdated` (=7) — also closes the
earlier stale-installed-skill propagation gap. Spec:
/tmp/oyo-outdated-tab-mr-naming-from-claude.md. Victor: clicking a sidebar outdated
comment navigates to the diff but shows NO card (inline overlay excludes all
outdated, review.rs:3287, no focus exception) — dead-end. Fix (Part A): a dedicated
"Outdated comments" tab/view mirroring the PR-comments tab (outdated comments have
stale anchors, can't render inline reliably, same rationale as PR conversation
comments getting their own tab). Mirror `TopbarTabContent` (mod.rs:168, add
`OutdatedComments`), label (ui.rs:1980), match arms, `open_outdated_comments_tab`,
`render_outdated_comments_view` (mirror render_pr_comments_view ui.rs:3517) —
each card shows author + original file:line + body + the captured `anchorSnapshot`
(line+context) + "Outdated" tag, with resolve/delete actions. Clicking a sidebar
outdated comment opens this tab focused on it (fixes the dead-end) + a key to open.
Part B (provider-aware naming): a `ReviewProviderKind` helper → PR/MR + pull
request/merge request (GitLab→MR, GitHub/Forgejo→PR) used for the PR-comments tab
label (ui.rs:1980) + copy; keyed off the review provider. GitLab not wired yet (P4)
so reads PR today — future-proofing only, don't wire GitLab.

## P2 — Feature: confirmQuit (confirm before quitting the TUI)

Status: DONE & VERIFIED (code/test; TUI feel pending user). Spec:
/tmp/oyo-confirm-quit-from-claude.md. Verified: `confirm_quit` default true
(config.rs:801, config test), modal title "Quit oyo?" + body "Are you sure you want
to quit?" (ui.rs:6528-6529, render test asserts both), Enter/y/q confirm & Esc/n/other
cancel, overlays/pickers/editors/path-popup close first, Ctrl+C force-quit bypass,
`confirm_quit=false` immediate, CONFIG.md documents it (row + example), delete modal
shares the refactored renderer; 7 quit tests pass (330 total).
`confirm_quit: bool` in UiConfig (config.rs:724), DEFAULT true. Quit action
(`NormalAction::Quit`, q/esc, keybindings.rs:277/input.rs:723) shows a
"Quit oyo?" confirm modal (reuse the `ReviewDeleteConfirmation` infra, app/mod.rs:862)
when it would exit and confirm_quit is on; Enter/y confirms, Esc/n cancels;
confirm_quit=false quits immediately. Esc/q still close overlays first (confirm only
when actually exiting); Ctrl+C stays a force-quit bypass. Modal has a TITLE ("Quit
oyo?") AND a body message ("Are you sure you want to quit?") then confirm/cancel — a
full confirmation dialog, not a bare title line. Optional: confirm on unsaved
editor. Document in docs/CONFIG.md.

## P2 — Bug: wrapped code-block continuation row has transparent trailing cells

Status: DONE & VERIFIED. Wrapped review-card code blocks now carry the code-panel bg
through every trailing cell on continuation rows (fill only when a row actually wraps
and all spans share the same non-transparent bg — mixed markdown untouched; short
panels stay shrink-wrapped). Test `wrapped_snapshot_code_background_fills_the_
continuation_row` passes (checks the continuation row through the right edge + short
panel leaves view bg outside); 331 tests. (Also landed Victor copy tweaks: quit modal
title → "Quit", "Captured snapshot:" → dimmed "Snapshot" label.) Spec:
/tmp/oyo-codeblock-wrap-bg-from-claude.md. Reported by Victor on the Outdated view
snapshot code block; likely general to code blocks. When a code-block line WRAPS, the
continuation row's trailing cells (right side) show the VIEW bg instead of the
code-block bg. Cause: `apply_line_bg` (views/mod.rs:973) full-width-pads the bg ONLY
when `!line_wrap`; with wrapping on it relies on the separate `push_wrapped_bg_line`
layer (:1008) whose wrap_count×wrap_width must match the wrapped text — but the
snapshot's `→ ` marker/2-space indent (review_snapshot_code_spans, :426) shifts the
effective width, so a continuation row is left uncovered. Fix: every wrapped code-block
row gets the code bg to full wrap width (reconcile the bg layer with the actual wrapped
text incl. the prefix, or pad each wrapped row directly); fix for code blocks generally.

## P3 — Review card overflow menu (…) for secondary actions

Status: DONE — code-verified; visual/interaction pending user confirm in the TUI.
Implemented via `ReviewCommentContextMenuAction { Body, Id, FileLine, Url,
MarkdownQuote }` + `ReviewCommentContextMenu { comment_id, x, y }` (mirrors
`FileContextMenu`); `o<card>` … affordance + hotkey; clipboard via existing
`copy_to_clipboard`. Code-verified: menu enum, `review_comment_path_line_label`
produces `~/…:R2` (regression test asserts `{}/src/lib.rs:R2`), Url pushed only
for github provider (test asserts absent on local), no CLI/`--json` change.
STILL TO CONFIRM IN TUI: `…` renders on cards, click opens / Esc + click-away
close, each item copies, PR-URL present only on pulled provider comments.
Follow-up sent: rename menu label "Copy path + line" → "Copy location" (matches the
`Location:` field; copied value `~/…:R2` unchanged).
Spec: /tmp/oyo-review-card-overflow-menu-from-claude.md. Mirror the existing
`FileContextMenu`/`FileContextMenuAction`/`FileContextMenuHit` pattern
(`app/mod.rs:99-117`, `:360`) — a `ReviewCommentContextMenu { comment_id, x, y }` +
action enum + hit-testing, same open/hit/close/render lifecycle. Actions (final): copy
body; copy id (`#N`); **copy path + line** — a SINGLE item copying
`~/repo/file.txt:R2` (home-collapsed absolute path + oyo's side-aware location
label via `review_anchor_location_label`, anchor-kind-aware: `path:R2` for a line,
range start for a hunk, bare `path` for a file comment; no separate plain "Copy
path"); copy PR URL (only when `comment.provider` has a link — omit for local);
optional copy-as-blockquote.
Mouse `…` click + a per-card hotkey in the `i`/`r`/`x` style (`review.rs:370-372`).
Clipboard-only, reuse the existing `CopyPath` clipboard path, TUI-only (no CLI /
`--json` change). PR conversation card minimal/excluded like GitHub.

Add a unicode `…` (kebab/overflow) affordance on the review card. Clicking it
opens a small dropdown of secondary actions, keeping the card clean while the
common ones (edit / delete / resolve) stay inline. Reuse oyo's existing
context-menu infrastructure (right-click menus).

Actions:
- Copy comment body
- Copy comment id
- Copy PR URL / permalink — **only when it is a PR comment with a link**
- (maybe) Copy `file:line`, copy as a Markdown blockquote (reply-style)

Give it a per-card hotkey too, consistent with `ia`/`xa`/`ra`, plus mouse click
on the `…`.

## P1 — Sync resolve/unresolve to GitHub (PR review threads)

Status: DONE. GitHub pull augments REST comments with GraphQL review thread ids and
resolved state. Local resolve changes apply to every comment in the thread. Push
uses `resolveReviewThread` or `unresolveReviewThread` once per changed thread and
updates each local provider baseline. Conversation comments and local-only comments
stay local. Failed thread permissions warn without blocking comment changes. Tests
cover GraphQL mapping, import, resolve, unresolve, deduplication, local propagation,
pending state, pull conflicts and in-flight toggles. Spec:
/tmp/oyo-resolve-sync-github-from-claude.md.
Verified (code/test): `gh_review_thread_states` (main.rs:3282) paginates GraphQL
`reviewThreads` mapping `fullDatabaseId`→thread id + `isResolved`;
`github_set_review_thread_resolved` (3651) calls the mutations; `pending_threads`
BTreeMap dedupes one mutation/thread; `api_kind=="issue"` excluded; 311+47+2 tests
pass. LIVE ROUND-TRIP VERIFIED against real PR
(ahkohd/oyo-review-dogfood-1783482731 #1): pull imported 3 review-thread comments
with real `PRRT_…` thread ids + resolved=false (and 4 conversation `kind=issue`
comments correctly with no thread id); `resolve` → `push` (1 thread update) → the
README thread flipped to `isResolved=true` on GitHub; `unresolve` → `push` flipped
it back. PR restored to original (all unresolved). One thread update per push
(dedupe) confirmed. Done, live-verified.

## P1 — Bug: `oy review` subcommands disagree on the default target (silent agent no-op)

Status: DONE & LIVE-VERIFIED (Claude, real PR #1). Spec:
/tmp/oyo-review-target-ux-from-claude.md. Every target-bearing review command now
uses one stateless resolver. It prefers the current branch's saved local PR review,
falls back to the worktree, and makes no provider call. `-t/--target` works on all
commands and comment actions; positional forms remain compatible. Human output starts
with `Reviewing:`. JSON always includes `reviewKey`, `target`, `label` and `pr`.
Explicit `-t @` still selects the worktree. PR #1 dogfood confirmed bare status and
comment load all 7 pulled comments under the same branch review key; a copied review
DB confirmed bare resolve updates that same review. The installed skill tells agents
to verify JSON target fields and pin with `-t` when needed.
Claude live-verified on real PR #1: bare `comment --unresolved`=7 (was 0),
`Reviewing: PR #1 (feature/review-sync)` header, `--json` reviewKey+label+pr non-null,
`-t @`=empty worktree, bare `resolve 1` works, `-t` uniform on comment+resolve, and
GitHub left untouched (local-only ops restored). Silent agent no-op fixed.

## P1 — Feature: reply to a pulled PR review-thread comment (synced) — Phase 1

Status: DONE & LIVE-VERIFIED (CLI, real PR #1; TUI `p` action pending user). Shared
App method for both surfaces. CLI-verified live: `oy review comment reply 3 --body …`
→ local child #5 (`inReplyTo=3547570223`, author from OYO_REVIEW_AUTHOR) → push
(1 created) → GitHub thread PZKV gained a 2nd comment IN THE SAME review thread →
re-pull round-tripped (no dup cards) → deleted to restore the PR. 350 tests pass; docs
+ installed skill updated. Also: mixed-reply-failure preserves successful ids +
retries only failures; pull refreshes pending reply thread state without losing local
resolve intent.

Spec:
/tmp/oyo-reply-diff-comment-phase1-from-claude.md. Completes the PR-review loop after
resolve-sync — diff/inline comments currently have NO reply (only PR conversation
comments do, via blockquote). Phase 1: reply to a pulled PR REVIEW-thread comment
(api_kind==review, has thread_id) → a new local comment carrying thread_id +
in_reply_to=parent comment_id, rendered nested under the parent, synced on push as a
GitHub review-thread reply (POST .../pulls/{pr}/comments/{parent_id}/replies or create
w/ in_reply_to). Builds on: in_reply_to_id captured on pull (main.rs:3161),
github_create_comment (3768), the conversation-reply UI to mirror
(start_pull_request_reply review.rs:2512), review_thread_key grouping (review.rs:146).
OUT: conversation comments (already blockquote-reply), local-only inline (Phase 2 =
local threading). CLI-FIRST (amendment): `oy review comment reply <parent_id>
--body "..."` — symmetric with resolve/edit, same threaded child, synced on push,
respects `-t`/PR-aware default + `OYO_REVIEW_AUTHOR_*` attribution + `--json`, errors
if parent isn't a pulled review-thread comment; documented in REVIEW.md + SKILL. So an
agent can reply to the human and a human can reply to a teammate from the CLI (not just
TUI). Will live-verify on PR #1.

## P1 — Feature: reply first-class on every inline card + card-action re-letter

Status: DONE & LIVE-VERIFIED (core; FLAT render pending). `reply_label`/`resolve_label`
now `r`/`v` (review.rs:4008/4010); `in_reply_to` local threading field; reply ungated
on every inline card. LIVE-verified: LOCAL reply (CLI `reply 1` → child inReplyTo=1, no
provider, stays local, human↔agent authorship) AND PULLED reply on PR #1 (reply → push
1 created → GitHub thread gained it → restored). 354 tests pass. PENDING: oy-gpt
rendered threads NESTED/indented, but Victor approved FLAT (GitHub-style, no nesting) —
flat render DONE & VERIFIED: `overlay.thread_continues` appends a one-space-inset muted
`│` (views/mod.rs:708), flat stack at one margin; test
`local_reply_thread_is_flat_with_a_connector` passes.

Spec:
/tmp/oyo-reply-first-class-from-claude.md. Supersedes the Phase-1 GitHub gating.
Part A (universal reply / local threading): reply on EVERY inline comment — LOCAL
parent → local nested child (in_reply_to=parent local id, no provider, stays local,
the human↔agent line conversation); PULLED PR parent → synced child (Phase 1). Ungate
`inline_review_reply_available` (review.rs:2364); CLI `reply <parent_id>` accepts local
parents (add_review_reply_from_cli review.rs:2665). Nested render grouped by parent
(local) / thread_id (pulled). Part B (mnemonic re-scheme): `ia edit · ra reply · va
resolve · xa delete · oa overflow` — reply `p`→`r` (review.rs:3911), resolve `r`→`v`
(3913/3962); CLI verbs unchanged, TUI letters + footer + docs/skill/KEYBINDINGS updated.
Victor picked `va` for resolve.

## P2 — Three search/quit UX fixes (split highlight, Esc closes find bar, Esc off quit)

Status: DONE & VERIFIED (code/test; TUI visual pending user). All three:
(1) split active-match coordinate fixed (accent+bold through next/prev/wrap; 11 search
tests); (2) `escape_closes_search_after_accept_without_quitting` test — Esc clears the
retained find bar, no quit modal; (3) `Quit => ["q"]` (keybindings.rs:277), esc removed
from app-quit (history/help/pickers keep Esc-to-close); 9 quit tests. KEYBINDINGS+README
updated. 357 tests.
Spec:
/tmp/oyo-search-quit-ux-from-claude.md. Reported by Victor.
(1) SPLIT view shows NO active-match highlight (unified fine): split computes
`is_active_match = search_target()==Some(display_idx)` (split.rs:1325/2363) + calls
highlight_search_spans, but it's never true — split render `display_idx` is in a
different coordinate space than `search_target` (from collect_search_matches' split
branch); matches dim, never bold-active. Fix: align split's active-match coordinate
with search_target.
(2) Esc should CLOSE the find bar when cycling (post-Enter, bar visible, search_active
false) — currently falls through to Quit (confirm modal). Make Esc dismiss search
(stop_search) whenever the search bar is visible.
(3) Remove Esc from QUIT — only `q` quits by default: keybindings.rs:277
`Quit => [q, esc]` → `[q]`. Keep sub-view Esc-to-close (history/help). Update
KEYBINDINGS docs. With 2+3, Esc = pure cancel/close; `q` = the only app-quit (w/ confirmQuit).

## P4 — More review providers (GitLab, Forgejo / Codeberg)

Status: proposed (LEAST priority)

Today only GitHub (`gh`) is supported for `pull`/`push`. Add adapters via the
existing provider contract (`[review.providers.<id>]`): **GitLab** (`glab`) and
**Forgejo/Codeberg** (`fj`, one adapter covers both). The provider interface
already exists, so these are new adapters, not new plumbing.
