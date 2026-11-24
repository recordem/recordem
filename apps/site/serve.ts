// The landing page in production: the prerendered files in dist/. Hashed assets are cached for a year; the page never.
const DIST = new URL("./dist/", import.meta.url).pathname;
const port = Number(process.env.SITE_PORT ?? 39415);

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
        return new Response(f, { headers: { "cache-control": immutable ? "public, max-age=31536000, immutable" : "public, max-age=3600" } });
      }
      // An unknown path is a 404, not a redirect home: crawlers index nothing that isn't there.
      return new Response("Not found", { status: 404, headers: { "content-type": "text/plain; charset=utf-8" } });
    }
    return new Response(Bun.file(DIST + "index.html"), { headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-cache" } });
  },
});
console.log(`site on :${port}`);
