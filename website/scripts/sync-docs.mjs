// Syncs the repo's docs/*.md into the Starlight content collection.
//
// docs/ stays the single source of truth (its links keep working on GitHub).
// This script derives a page title, strips the leading H1 (Starlight renders
// the title itself), rewrites cross-links to site slugs, and co-locates the
// screenshot assets so images resolve. Run automatically before dev/build.

import { fileURLToPath } from "node:url";
import fs from "node:fs";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..", "..");
const docsDir = path.join(repoRoot, "docs");
const assetsDir = path.join(repoRoot, "assets");
const outDir = path.resolve(here, "..", "src", "content", "docs");

// Curated, user-facing docs. Internal notes (DEBUG, PERF, PROFILING) and the
// MARKDOWN preview fixture are intentionally left off the public site.
const PAGES = [
  { file: "CONFIG.md",        slug: "config",        title: "Configure",             group: "guide", order: 1 },
  { file: "THEME.md",         slug: "theming",       title: "Theming",               group: "guide", order: 2 },
  { file: "KEYBINDINGS.md",   slug: "keybindings",   title: "Keybindings",           group: "guide", order: 3 },
  { file: "REVIEW.md",        slug: "review",        title: "Review",                group: "guide", order: 4 },
  { file: "CONTROL.md",       slug: "control",       title: "Control",               group: "guide", order: 5 },
  { file: "AGENT.md",         slug: "agents",        title: "Working with agents",   group: "guide", order: 6 },
  { file: "SKILL.md",         source: "../crates/oyo/docs/SKILL.md", slug: "review-skill", title: "Oyo code review skill", group: "guide", order: 7 },
  { file: "CONTROL_SKILL.md", source: "../crates/oyo/docs/CONTROL.md", slug: "control-skill", title: "Oyo TUI control skill", group: "guide", order: 8 },
  { file: "WALKTHROUGH.md",   source: "../crates/oyo/docs/WALKTHROUGH.md", slug: "walkthrough-skill", title: "Oyo code walkthrough skill", group: "guide", order: 9 },
  { file: "REVIEW_HOOKS.md",  slug: "hooks",         title: "Hooks",                 group: "guide", order: 10 },
  { file: "DIFF_VIEWER.md",   slug: "diff-viewer",   title: "Diff Viewer Behaviour", group: "ref",   order: 1 },
  { file: "DIFF_PREVIEWS.md", slug: "diff-styling",  title: "Diff Styling Previews", group: "ref",   order: 2 },
];

const slugByFile = new Map(PAGES.map((p) => [p.file, p.slug]));

function rewrite(md) {
  // Cross-links: [text](./THEME.md#anchor) -> [text](../theming/#anchor)
  md = md.replace(
    /\]\((?:\.\/)?([A-Za-z0-9_]+)\.md(#[A-Za-z0-9_-]+)?\)/g,
    (whole, base, anchor = "") => {
      const slug = slugByFile.get(`${base}.md`);
      if (!slug) return whole; // leave unknown targets untouched
      return `](../${slug}/${anchor})`;
    },
  );
  // Skill link: [text](../crates/oyo/docs/SKILL.md) -> [text](../review-skill/)
  md = md.replace(/\]\(\.\.\/crates\/oyo\/docs\/SKILL\.md\)/g, "](../review-skill/)");
  // Screenshot images: ![alt](../assets/x.png) -> co-located ![alt](./assets/x.png)
  md = md.replace(/\]\(\.\.\/assets\//g, "](./assets/");
  return md;
}

function frontmatter({ title }) {
  return `---\ntitle: "${title.replace(/"/g, '\\"')}"\nbanner:\n  content: 'Oyo is under active development. Expect rough edges and bugs in current 0.1 releases. <a href="https://github.com/ahkohd/oyo/issues/new">Report problems on GitHub.</a>'\n---\n\n`;
}

// Clean previous output, then rebuild.
fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(outDir, { recursive: true });

let needAssets = false;
for (const page of PAGES) {
  const srcPath = page.source ? path.resolve(docsDir, page.source) : path.join(docsDir, page.file);
  if (!fs.existsSync(srcPath)) {
    console.warn(`sync-docs: missing ${page.file}, skipping`);
    continue;
  }
  let body = fs.readFileSync(srcPath, "utf8");
  // Skill files have their own agent metadata. Starlight only needs the page title.
  body = body.replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n+/, "");
  // Drop the leading "# Title" heading. Starlight renders the frontmatter title.
  body = body.replace(/^#\s+.+\r?\n+/, "");
  body = rewrite(body);
  if (body.includes("./assets/")) needAssets = true;
  fs.writeFileSync(path.join(outDir, `${page.slug}.md`), frontmatter(page) + body);
}

// Co-locate screenshots referenced by the previews page.
if (needAssets && fs.existsSync(assetsDir)) {
  const dest = path.join(outDir, "assets");
  fs.mkdirSync(dest, { recursive: true });
  for (const f of fs.readdirSync(assetsDir)) {
    if (/\.(png|jpe?g|gif|svg|webp)$/i.test(f)) {
      fs.copyFileSync(path.join(assetsDir, f), path.join(dest, f));
    }
  }
}

console.log(`sync-docs: wrote ${PAGES.length} pages to src/content/docs/`);

// Vendor the self-contained asciinema-player bundle into public/ for the hero
// demo. The standalone bundle inlines its web worker, so it's safe under the
// site's base path (no bundler/worker path rewriting needed).
const playerDist = path.resolve(
  here,
  "..",
  "node_modules",
  "asciinema-player",
  "dist",
  "bundle",
);
const vendorDir = path.resolve(here, "..", "public", "vendor");
if (fs.existsSync(playerDist)) {
  fs.mkdirSync(vendorDir, { recursive: true });
  for (const f of ["asciinema-player.min.js", "asciinema-player.css"]) {
    fs.copyFileSync(path.join(playerDist, f), path.join(vendorDir, f));
  }
  console.log("sync-docs: vendored asciinema-player bundle to public/vendor/");
}

