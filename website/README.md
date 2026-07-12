# Oyo website

The marketing landing page and documentation site for [Oyo](https://github.com/ahkohd/oyo),
built with [Astro](https://astro.build) and [Starlight](https://starlight.astro.build)
and deployed to GitHub Pages.

## How it works

- The landing page lives in `src/pages/index.astro` (styles in `src/styles/landing.css`).
- The docs are not authored here. `scripts/sync-docs.mjs` pulls the curated
  `docs/*.md` files from the repo root, adds titles, rewrites cross-links to site
  slugs, and co-locates screenshots. It runs automatically before `dev` and `build`.
  Edit the docs in `../docs/`, not in `src/content/docs/` (that folder is generated
  and git-ignored).

## Develop

```sh
cd website
npm install
npm run dev      # runs sync-docs, then astro dev
```

Open the printed URL (served under the `/oyo` base path).

## Build

```sh
npm run build    # writes website/dist
npm run preview
```

## Deploy

Pushing to `main` with changes under `website/**`, `docs/**`, or `assets/**`
triggers `.github/workflows/deploy-website.yml`, which builds and publishes to
GitHub Pages.

One-time setup: in the repo settings, set Pages, Build and deployment, Source to GitHub Actions.

## Custom domain

The site is configured for the project-page URL `https://ahkohd.github.io/oyo/`.
To use a custom domain, set `site` to it and `base` to `"/"` in `astro.config.mjs`,
and add a `public/CNAME` file.
