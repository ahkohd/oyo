import { pages, markdownPath, documentFor } from "../../scripts/docs.mjs";

export function getStaticPaths() {
  return pages.map((page) => ({ params: { doc: markdownPath(page).slice(0, -3) }, props: { page } }));
}

export function GET({ props, site }) {
  const base = `${import.meta.env.BASE_URL.replace(/\/$/, "")}/`;
  const { body } = documentFor(props.page, { markdown: true, site, base });
  return new Response(body, { headers: { "Content-Type": "text/markdown; charset=utf-8" } });
}
