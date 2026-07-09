# Oyo — product essence

Marketing context for the website. What Oyo is, who it's for, and why it matters. Written to keep landing copy honest and cohesive (see the copywriting skill).

## One line

**Oyo is a complete code-review tool that lives in your terminal — and because it's terminal-native and CLI-driven, it's the first review surface a coding agent can genuinely share with you.**

Binary: `oy`. It's a diff viewer *and* a review tool.

## Why now

Coding agents write a large share of the code today, so the bottleneck has shifted from *writing* to *reviewing*. But review has always been human-only and browser-bound — the GitHub PR tab. That breaks the moment an agent is involved:

- The agent lives in the terminal; the review lives in a browser it can't touch.
- So the agent "reviews" by dumping text into a chat, or a tool fakes keystrokes into your UI. Neither is real participation.
- You context-switch constantly: terminal → code, browser → review, back again.

Oyo's bet: bring review *into the terminal*, as a surface both sides can actually read from and write to.

## The core insight

One property — **terminal-native + CLI-driven** — delivers *both* audiences from a single root:

- It makes Oyo a fast, full-featured review tool for **any developer** (solo or with a team), no browser required.
- It makes the review a real, structured API (`oy review`, `oy control`) that an **agent** can join as a peer.

You don't choose an audience. The same design serves a solo dev, a team, and a human+agent pair.

## Who it's for

- **Developers who work with coding agents** (primary). Harness on the left, Oyo on the right.
- **Any developer who reviews code** and would rather not leave the terminal.
- **Teams** who review on GitHub pull requests but want to do the reading, commenting and resolving locally.

## What it does (grounded in the code)

**As a diff viewer**
- Two ways to read: scroll freely, or step through a change one edit at a time (`--step`).
- Five views: unified, split, evolution (animated replay), blame, preview.
- Rich previews inside the diff: Markdown, JSON, YAML, TOML, CSV, images.
- Word-level (intra-line) diffing, hunk/file/step navigation, regex search, tabs (each with its own view), mouse support, watch mode, zen mode.
- Works with **Git and Jujutsu**; drops in as a difftool.

**As a review tool**
- Inline comments on any line (new/old side), whole hunks, whole files, or the PR itself.
- Comment cards render **Markdown** (headings, code blocks, task lists, tables, inline images).
- Resolve or reopen threads; edit; delete; reply inline in local or provider threads. Pull request conversation replies quote the parent as a Markdown blockquote.
- Comments **re-anchor as the diff changes** and flag themselves when an edit leaves them stale/outdated.
- Every comment carries an **author identity** — human, agent or bot — shown on the card, with avatars.
- @-mentions in the comment editor.
- Review **any target**: working tree, staged, a commit, a branch, a `base...feature` PR range, or a jj change/bookmark/revset.
- Comments are stored **locally** (SQLite, per workspace, in the OS data dir) — private drafts until you choose to sync. NOTE: not stored in git; don't call it "git-backed."
- **Sync with a pull request**: pull a PR's inline + conversation comments into the terminal, push your own back. GitHub via `gh` is the **only live provider today**; GitLab/Codeberg/Forgejo and a custom-provider contract are planned/partial. NOTE: PR conversation threads can't be "resolved" from Oyo, and resolve state never syncs — resolve is a local-only toggle.
- Export the whole review to Markdown or JSON.

**As an agent surface**
- `oy review` — a git-shaped CLI to read and write the same comments an agent already understands. An agent can read unresolved comments as a task list, leave its own findings, resolve what it fixed, or comment on your behalf.
- `oy control` — steer a *running* Oyo TUI from another process: open files, jump to a line, change target, switch views, and **play a guided walk-through** (step mode, one edit at a time). Crucially, the agent drives **real app state, not fake keystrokes**; the user's input always preempts it, so the human stays in control.
- Review hooks fire on events (`comment_saved`, etc.) to wire Oyo into other tools.

## The signature workflow (voice of customer)

Coding harness on the left, Oyo on the right. The agent finishes and drops line comments. You annotate while it's still working. You say "check comment 12" — it reads it, fixes it, marks it resolved. Or you ask it to review your code and it leaves comments line by line. When teammates review on GitHub, you pull their comments in, resolve locally, and push yours up — never opening a browser.

## Differentiators (what to lead with)

1. **A real review tool in the terminal** — not a diff viewer with a comment bolted on. Everything you'd expect from code review, no browser.
2. **Shared with your agent** — the only review surface an agent can genuinely participate in (real CLIs, real app state, one shared comment thread).
3. **Meets your VCS where it is** — any Git/jj target, synced to your pull requests.

## Voice and tone

Developer audience. Plain, confident, active. Show the outcome, not adjectives. No exclamation points, no buzzwords ("streamline", "seamless", "revolutionary"). Mirror how developers actually talk ("harness on the left, Oyo on the right", "check comment 12"). Be honest — every claim maps to a real feature; avoid the two known traps ("git-backed", PR "resolve").

## Accuracy guardrails

- Don't say comments are "git-backed" or "live with your branch" — they're a local SQLite DB.
- Don't claim you can resolve or sync-resolve PR conversation threads.
- GitHub is the only live sync provider today; others are planned.
- "One edit at a time" walk-throughs mean **step mode**, not raw scrolling.
