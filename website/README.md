# Oyo website

The landing page and documentation site for [Oyo](https://github.com/ahkohd/oyo) use Astro and Starlight. Netlify publishes the site at [oyo.victorare.mu](https://oyo.victorare.mu/).

## Edit the site

The landing page is `src/pages/index.astro`. Shared landing styles are in `src/styles/landing.css`.

Edit documentation in `../docs/`. `scripts/sync-docs.mjs` copies the curated pages into the ignored `src/content/docs/` directory before development and builds. Add a page to that script and the sidebar in `astro.config.mjs` to publish it.

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

The build writes `website/dist`, including the setup guide and starter preset. Preview serves that static output.

## Deploy

The root `netlify.toml` sets the base directory to `website`, runs `npm run build`, and publishes `dist`. The website build includes changes from `docs/` through the sync script. Publication follows the project's Netlify configuration.
