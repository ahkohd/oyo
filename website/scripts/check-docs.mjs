import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkGfm from "remark-gfm";
import { pages, sourcePath, readSource, markdownPath, visitLinks } from "./docs.mjs";

const dist = path.resolve(process.argv[2] ?? "dist");
const base = `/${(process.argv[3] ?? "/").replace(/^\/+|\/+$/g, "")}/`.replace(/^\/\/$/, "/");
const origin = "https://oyo.victorare.mu";
const parser = unified().use(remarkParse).use(remarkGfm);
const read = (relative) => fs.readFileSync(path.join(dist, relative), "utf8");
const canonical = (relative) => new URL(`${base}${relative}`, origin).href;
const llms = read("llms.txt");
const llmsTree = parser.parse(llms);
const llmsLinks = [];
visitLinks(llmsTree, (node) => llmsLinks.push(node.url));
assert.ok(llms.startsWith("# Oyo\n"));
assert.ok(Buffer.byteLength(llms) < 5000, "Keep llms.txt concise");
assert.deepEqual(new Set(llmsLinks), new Set(pages.map((page) => canonical(markdownPath(page)))));
assert.equal(llmsTree.children.filter((node) => node.type === "heading" && node.depth === 1).length, 1);
assert.ok(!fs.existsSync(path.join(dist, "llms-full.txt")));
assert.equal(read("setup.md"), fs.readFileSync(path.resolve("../docs/SETUP.md"), "utf8"));
assert.equal(read("starter.toml"), fs.readFileSync(path.resolve("../docs/STARTER.toml"), "utf8"));

function content(node) {
  if (Array.isArray(node)) return node.map(content);
  if (node && typeof node === "object") {
    return Object.fromEntries(Object.entries(node).filter(([key]) => !["position", "url"].includes(key))
      .map(([key, value]) => [key, content(value)]));
  }
  return node;
}

let localLinks = 0;
for (const page of pages) {
  const markdown = read(markdownPath(page));
  const tree = parser.parse(markdown);
  assert.deepEqual(content(tree), content(parser.parse(readSource(page))), `Source content drift: ${sourcePath(page)}`);
  const html = read(`${page.slug}/index.html`);
  assert.ok(html.includes(`rel="alternate" type="text/markdown" href="${canonical(markdownPath(page))}"`), `Missing alternate: ${page.slug}`);
  assert.ok(html.includes(`rel="describedby" href="${canonical("llms.txt")}"`), `Missing describedby: ${page.slug}`);
  assert.ok(html.includes(`rel="canonical" href="${canonical(`${page.slug}/`)}"`), `Canonical origin drift: ${page.slug}`);
  visitLinks(tree, (node) => {
    const url = new URL(node.url, canonical(markdownPath(page)));
    if (url.origin !== origin) return;
    assert.ok(url.pathname.startsWith(base), `Link escapes base: ${node.url}`);
    const relative = decodeURIComponent(url.pathname.slice(base.length));
    const target = path.join(dist, !relative || relative.endsWith("/") ? `${relative}index.html` : relative);
    assert.ok(fs.existsSync(target), `Missing link target from ${page.slug}: ${node.url}`);
    if (url.hash) {
      const htmlTarget = target.endsWith(".md") ? target.replace(/index\.md$/, "index.html") : target;
      assert.ok(fs.readFileSync(htmlTarget, "utf8").includes(`id="${decodeURIComponent(url.hash.slice(1))}"`), `Missing anchor from ${page.slug}: ${node.url}`);
    }
    localLinks++;
  });
}
assert.ok(read("index.html").includes(`rel="describedby" href="${canonical("llms.txt")}"`));
for (const name of ["DEBUG", "PERF", "PROFILING", "MARKDOWN"]) {
  assert.ok(!pages.some((page) => page.file === `${name}.md`), `Internal document published: ${name}`);
}
console.log(`check-docs: ${pages.length} source-derived Markdown pages, ${localLinks} local links and anchors, discovery and canonical URLs passed at ${base}`);
