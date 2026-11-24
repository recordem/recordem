// The landing page (recordem.dev). `bun run build` builds the client, then renders the page once on the server
// (prerender.ts) so dist/index.html carries the full markup; the client only hydrates it for the live demo and tabs.
import react from "@vitejs/plugin-react";
import { defineConfig, type Plugin } from "vite";

// Umami (the hosted site's analytics, no cookies): on only when the build is given a website id, and only on
// recordem.dev, so dev servers and self-hosted copies of the page send nothing.
const UMAMI_SRC = process.env.UMAMI_SRC?.trim() || "https://analytics.rklosowski.com/umami";
const UMAMI_WEBSITE_ID = process.env.UMAMI_WEBSITE_ID?.trim();
const UMAMI_DOMAINS = process.env.UMAMI_DOMAINS?.trim() || "recordem.dev,www.recordem.dev";
const umami = (): Plugin => ({
  name: "umami",
  transformIndexHtml: () =>
    UMAMI_WEBSITE_ID
      ? [{ tag: "script", attrs: { defer: true, src: UMAMI_SRC, "data-website-id": UMAMI_WEBSITE_ID, "data-domains": UMAMI_DOMAINS }, injectTo: "head" }]
      : [],
});

export default defineConfig({
  plugins: [react(), umami()],
  server: { host: "0.0.0.0", port: Number(process.env.SITE_PORT ?? 39415), strictPort: true },
  preview: { host: "0.0.0.0", port: Number(process.env.SITE_PORT ?? 39415), strictPort: true },
});
