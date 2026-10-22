// The landing page (recordem.dev). `bun run build` builds the client, then renders the page once on the server
// (prerender.ts) so dist/index.html carries the full markup; the client only hydrates it for the live demo and tabs.
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  plugins: [react()],
  server: { host: "0.0.0.0", port: Number(process.env.SITE_PORT ?? 39415), strictPort: true },
  preview: { host: "0.0.0.0", port: Number(process.env.SITE_PORT ?? 39415), strictPort: true },
});
