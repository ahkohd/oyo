import fs from "node:fs";
import path from "node:path";
import { unified } from "unified";
import remarkParse from "remark-parse";
import remarkStringify from "remark-stringify";
import remarkGfm from "remark-gfm";

export const repoRoot = path.resolve(process.cwd(), "..");
export const pages = [
  { file: "SETUP.md", slug: "setup", title: "Set up Oyo with an agent", description: "Install Oyo, find the active config, and safely merge the optional starter preset." },
  { file: "CONFIG.md", slug: "config", title: "Configure", description: "Config locations, settings, and provider command interfaces." },
  { file: "THEME.md", slug: "theming", title: "Theming", description: "UI and syntax themes, custom themes, and colour overrides.", optional: true },
  { file: "KEYBINDINGS.md", slug: "keybindings", title: "Keybindings", description: "Keyboard and mouse controls, modes, and custom bindings." },
  { file: "REVIEW.md", slug: "review", title: "Review", description: "Saved reviews, comments, targets, and provider synchronization." },
  { file: "REMOTE_DIFFS.md", slug: "remote-diffs", title: "Remote diffs", description: "Open review URLs without a checkout, public reads, authentication, and safe discussion synchronization." },
  { file: "CONTROL.md", slug: "control", title: "Control", description: "Inspect and control a running TUI session with oy control.", optional: true },
  { file: "AGENT.md", slug: "agents", title: "Working with agents", description: "Agent workflows for reviewing code, handling comments, and walkthroughs." },
  { file: "SKILL.md", source: "crates/oyo/docs/SKILL.md", slug: "review-skill", title: "Oyo code review skill", description: "Agent instructions for choosing targets and managing review comments." },
  { file: "CONTROL_SKILL.md", source: "crates/oyo/docs/CONTROL.md", slug: "control-skill", title: "Oyo TUI control skill", description: "Agent instructions for session selection and TUI control.", optional: true },
  { file: "WALKTHROUGH.md", source: "crates/oyo/docs/WALKTHROUGH.md", slug: "walkthrough-skill", title: "Oyo code walkthrough skill", description: "Agent instructions for walking through code in Oyo.", optional: true },
  { file: "REVIEW_HOOKS.md", slug: "hooks", title: "Hooks", description: "Review hooks, actions, events, and payloads.", optional: true },
  { file: "DIFF_VIEWER.md", slug: "diff-viewer", title: "Diff Viewer Behaviour", description: "Views, context folds, highlights, wrapping, and large diffs.", optional: true },
  { file: "DIFF_PREVIEWS.md", slug: "diff-styling", title: "Diff Styling Previews", description: "Screenshot examples of diff display settings.", optional: true },
];

export const sourcePath = (page) => page.source ?? `docs/${page.file}`;
export const markdownPath = (page) => `${page.slug}/index.md`;
const pageBySource = new Map(pages.map((page) => [sourcePath(page), page]));
const processor = unified().use(remarkParse).use(remarkGfm).use(remarkStringify);

export function visitLinks(node, visit) {
  if (typeof node.url === "string") visit(node);
  for (const child of node.children ?? []) visitLinks(child, visit);
}

export function readSource(page) {
  return fs.readFileSync(path.join(repoRoot, sourcePath(page)), "utf8")
    .replace(/^---\r?\n[\s\S]*?\r?\n---\r?\n+/, "");
}

export function documentFor(page, { markdown = false, site, base = "/" } = {}) {
  const tree = processor.parse(readSource(page));
  const assets = new Set();
  const pageLink = (target) => `../${markdown ? markdownPath(target) : `${target.slug}/`}`;
  visitLinks(tree, (node) => {
    const href = node.url;
    if (href.startsWith("#")) return;
    if (/^(?:[a-z][a-z\d+.-]*:|\/\/)/i.test(href)) {
      if (!markdown || !site) return;
      const url = new URL(href, site);
      if (url.origin !== site.origin) return;
      const target = pages.find((candidate) => [
        `/${candidate.slug}/`, `${base}${candidate.slug}/`,
        `/${markdownPath(candidate)}`, `${base}${markdownPath(candidate)}`,
      ].includes(url.pathname));
      node.url = target ? pageLink(target) + url.search + url.hash
        : new URL(`${base}${url.pathname.replace(/^\//, "")}${url.search}${url.hash}`, site).href;
      return;
    }
    const url = new URL(href, `https://source.invalid/${sourcePath(page)}`);
    const targetPath = decodeURIComponent(url.pathname.slice(1));
    const target = pageBySource.get(targetPath);
    if (target) {
      node.url = pageLink(target) + url.search + url.hash;
    } else if (targetPath.startsWith("assets/")) {
      assets.add(targetPath);
      node.url = `${markdown ? "../doc-assets/" : "./assets/"}${targetPath.slice(7)}${url.search}${url.hash}`;
    } else {
      if (targetPath.endsWith(".md")) throw new Error(`Uncurated documentation link in ${sourcePath(page)}: ${href}`);
      if (!fs.existsSync(path.join(repoRoot, targetPath))) throw new Error(`Missing source link in ${sourcePath(page)}: ${href}`);
      node.url = `https://raw.githubusercontent.com/ahkohd/oyo/main/${targetPath}${url.search}${url.hash}`;
    }
  });
  if (!markdown && tree.children[0]?.type === "heading" && tree.children[0].depth === 1) tree.children.shift();
  return { body: processor.stringify(tree), assets };
}
