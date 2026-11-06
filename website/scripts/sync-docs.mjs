// Syncs the repo's docs/*.md into the Starlight content collection.
//
// docs/ stays the single source of truth (its links keep working on GitHub).
// This script derives a page title, strips the leading H1 (Starlight renders
// the title itself), rewrites cross-links to site slugs, and co-locates the
// screenshot assets so images resolve. Run automatically before dev/build.

import { fileURLToPath } from "node:url";
import fs from "node:fs";
import path from "node:path";
import { pages, sourcePath, documentFor } from "./docs.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..", "..");
const docsDir = path.join(repoRoot, "docs");
const outDir = path.resolve(here, "..", "src", "content", "docs");

function frontmatter(page) {
  const { title } = page;
  const editPath = sourcePath(page);
  return `---\ntitle: "${title.replace(/"/g, '\\"')}"\neditUrl: "https://github.com/ahkohd/oyo/edit/main/${editPath}"\nbanner:\n  content: 'Oyo is under active development. Expect rough edges and bugs in current 0.1 releases. Feel free to <a href="https://github.com/ahkohd/oyo/issues/new">open an issue on GitHub.</a>'\n---\n\n`;
}

// Clean previous output, then rebuild.
fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(outDir, { recursive: true });

const publicAssets = path.resolve(here, "..", "public", "doc-assets");
fs.rmSync(publicAssets, { recursive: true, force: true });
for (const page of pages) {
  const { body, assets } = documentFor(page);
  fs.writeFileSync(path.join(outDir, `${page.slug}.md`), frontmatter(page) + body);
  for (const asset of assets) {
    const name = asset.slice("assets/".length);
    for (const dest of [path.join(outDir, "assets", name), path.join(publicAssets, name)]) {
      fs.mkdirSync(path.dirname(dest), { recursive: true });
      fs.copyFileSync(path.join(repoRoot, asset), dest);
    }
  }
}

console.log(`sync-docs: wrote ${pages.length} pages to src/content/docs/`);

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

for (const [source, target] of [["SETUP.md", "setup.md"], ["STARTER.toml", "starter.toml"]]) {
  fs.copyFileSync(path.join(docsDir, source), path.resolve(here, "..", "public", target));
}
