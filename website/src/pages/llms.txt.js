import { pages, markdownPath } from "../../scripts/docs.mjs";

export function GET({ site }) {
  const base = `${import.meta.env.BASE_URL.replace(/\/$/, "")}/`;
  const sections = [
    "# Oyo",
    "> Oyo is a terminal code review tool. Its executable is oy. Review local Git and Jujutsu changes or remote review URLs, manage comments, and work with coding agents.",
    "Start with the setup guide for installation and configuration. Public remote reviews support anonymous reads when the host allows access. Discussion writes require the provider's authentication.",
    ...[false, true].flatMap((optional) => [
      optional ? "## Optional" : "## Documentation",
      pages.filter((page) => Boolean(page.optional) === optional).map((page) =>
        `- [${page.title}](${new URL(`${base}${markdownPath(page)}`, site).href}): ${page.description}`,
      ).join("\n"),
    ]),
  ];
  return new Response(`${sections.join("\n\n")}\n`, { headers: { "Content-Type": "text/plain; charset=utf-8" } });
}
