// @ts-check
import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";
import react from "@astrojs/react";
import basicSsl from "@vitejs/plugin-basic-ssl";

// Only serve the dev server over HTTPS (self-signed) so a LAN IP is a secure
// context. The clipboard API (Agentation's copy, the install button) needs it.
const isDev = process.argv.includes("dev");

// Deployed to Netlify at the domain root. Update `site` to your actual Netlify
// URL (or custom domain) — it only affects canonical URLs and the sitemap.
export default defineConfig({
  site: "https://oyo.netlify.app",
  base: "/",
  vite: {
    // HTTPS on the dev server only; harmless / unused in the static build.
    plugins: isDev ? [basicSsl()] : [],
    server: {
      // Let the dev server accept requests proxied through these hosts,
      // otherwise it replies "Blocked request. This host is not allowed."
      allowedHosts: ["www.agentation.com", "agentation.com"],
    },
  },
  integrations: [
    react(),
    starlight({
      title: "Oyo",
      description:
        "A terminal diff viewer for stepping through changes and reviewing scrollable diffs.",
      customCss: [
        "@fontsource-variable/geist-mono",
        "./src/styles/starlight-theme.css",
      ],
      // Inject the dev-only Agentation toolbar on every docs page.
      components: {
        Footer: "./src/components/Footer.astro",
        SiteTitle: "./src/components/SiteTitle.astro",
        Header: "./src/components/Header.astro",
      },
      social: [
        { icon: "github", label: "GitHub", href: "https://github.com/ahkohd/oyo" },
      ],
      editLink: {
        baseUrl: "https://github.com/ahkohd/oyo/edit/main/docs/",
      },
      sidebar: [
        {
          label: "Guides",
          items: [
            {
              label: "Configuration",
              items: [
                { label: "Configure", slug: "config" },
                { label: "Theming", slug: "theming" },
                { label: "Keybindings", slug: "keybindings" },
                { label: "Hooks", slug: "hooks" },
              ],
            },
            {
              label: "CLI",
              items: [
                { label: "Review", slug: "review" },
                { label: "Control", slug: "control" },
              ],
            },
            { label: "Working With Agents", slug: "agents" },
          ],
        },
        {
          label: "Reference",
          items: [
            { label: "Diff Viewer Behaviour", slug: "diff-viewer" },
            { label: "Diff Styling Previews", slug: "diff-styling" },
          ],
        },
      ],
    }),
  ],
});
