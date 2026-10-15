// The portal in production: the built files (dist/), with every unknown path answered by index.html so the app's own
// routes (/m/<id>, /callback) load. Hashed assets are cached for a year; index.html never.
const DIST = new URL("./dist/", import.meta.url).pathname;
const port = Number(process.env.WEB_PORT ?? 39412);

Bun.serve({
  port,
  hostname: "0.0.0.0",
  async fetch(req) {
    const path = decodeURIComponent(new URL(req.url).pathname);
    if (path === "/healthz") return new Response("ok");
    if (!path.includes("..") && path !== "/") {
      const f = Bun.file(DIST + path.slice(1));
      if (await f.exists()) {
        const immutable = path.startsWith("/assets/");
        return new Response(f, { headers: { "cache-control": immutable ? "public, max-age=31536000, immutable" : "no-cache" } });
      }
    }
    return new Response(Bun.file(DIST + "index.html"), { headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-cache" } });
  },
});
console.log(`web on :${port}`);
