// Dev server for the portal. The API and the identity provider's token endpoint are proxied on the portal's own
// origin, so the browser talks to one https origin (OAuth requires https redirect URIs off loopback, and an https
// page can't call plain-http APIs). In production a reverse proxy does the same, or VITE_API_BASE points elsewhere.
import react from "@vitejs/plugin-react";
import { existsSync, readFileSync } from "node:fs";
import { defineConfig, loadEnv } from "vite";

export default defineConfig(({ mode }) => {
  const env = { ...loadEnv(mode, "../..", ""), ...process.env };
  const AUTH_URL = env.AUTH_URL ?? "http://localhost:39410";
  const API_URL = env.API_URL ?? "http://localhost:39411";
  const cert = ".cert/cert.pem", key = ".cert/key.pem";
  return {
    plugins: [react()],
    define: {
      __AUTH_URL__: JSON.stringify(AUTH_URL),          // where the browser is sent to sign in
      __API_AUDIENCE__: JSON.stringify(API_URL),       // the resource the tokens are for
      __API_BASE__: JSON.stringify(env.VITE_API_BASE ?? ""),   // "" = same origin, through the proxy
    },
    server: {
      host: "0.0.0.0",
      port: Number(env.WEB_PORT ?? 39412),
      strictPort: true,
      https: existsSync(cert) ? { cert: readFileSync(cert), key: readFileSync(key) } : undefined,
      proxy: {
        "/v1": { target: API_URL, xfwd: true },
        "/files": { target: API_URL, xfwd: true },
        "/idp": { target: AUTH_URL, rewrite: (p) => p.replace(/^\/idp/, "") },
      },
    },
  };
});
