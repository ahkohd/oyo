# Oyo website

The landing page and documentation site for [Oyo](https://github.com/ahkohd/oyo) use Astro and Starlight. Netlify publishes the site at [oyo.victorare.mu](https://oyo.victorare.mu/).

## Edit the site

The landing page is `src/pages/index.astro`. Shared landing styles are in `src/styles/landing.css`.

Edit documentation in `../docs/`. `scripts/sync-docs.mjs` copies the curated pages into the ignored `src/content/docs/` directory before development and builds. Add a page to the registry in `scripts/docs.mjs` and the sidebar in `astro.config.mjs` to publish it.

The same registry generates `/llms.txt` and clean Markdown alternates at `/<slug>/index.md`. The Markdown keeps the source headings and examples, rewrites curated doc links to their Markdown alternates, and serves referenced screenshots from `/doc-assets/`. Relative links to other repository files use GitHub raw URLs. Internal docs outside the registry are excluded.

Astro endpoints use the configured site URL and base path for the index. The Starlight `Head.astro` override adds Markdown alternate and llms.txt discovery links. The landing page links to llms.txt. `public/_headers` sets the Markdown content type for Netlify static hosting.

The setup guide is `../docs/SETUP.md`. Sync publishes it as both `/setup/` and the agent-readable `/setup.md`. It copies `../docs/STARTER.toml` to `/starter.toml`. The landing page's `SetupPrompt.astro` component links to that Markdown guide using the configured site URL. Keep that URL in `astro.config.mjs` aligned with the public domain.

## Develop

```sh
cd website
npm ci
npm run dev
```

Open the printed URL. The site uses the domain root, with no path prefix.

## Build and preview

```sh
npm run build
npm run preview
```

The build writes `website/dist`, including the setup guide and starter preset. Preview serves that static output. The build also runs `npm run check:docs` to compare generated Markdown content with its sources and verify local links, anchors, canonical URLs, and HTML discovery metadata. Run the same check against a build with a path prefix using `node scripts/check-docs.mjs <output-directory> <base-path>`. Run the scripts from `website/`.

## Deploy

The root `netlify.toml` sets the base directory to `website`, runs `npm run build`, and publishes `dist`. The website build includes changes from `docs/` through the sync script. Publication follows the project's Netlify configuration.
