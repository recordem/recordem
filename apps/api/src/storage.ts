// Where audio lives. S3-compatible (R2, AWS, Garage…) through Bun's own client when S3_BUCKET is set; otherwise a
// folder on disk, so a self-hosted server needs nothing else to start. Players get short-lived URLs: presigned by S3,
// or signed here (HMAC with API_SECRET) and served by `files` with Range support, so an <audio> element can seek.
import { createHmac, timingSafeEqual } from "node:crypto";
import { mkdir, rm } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

const SECRET = process.env.API_SECRET ?? (() => { throw new Error("API_SECRET is not set"); })();
const sign = (key: string, exp: number) => createHmac("sha256", SECRET).update(`${key}\n${exp}`).digest("base64url");

export interface Storage {
  put(key: string, body: Blob | ReadableStream, contentType: string): Promise<number>;
  /** A response that serves the object: a redirect to a short-lived URL (S3) or the bytes (disk). */
  serve(key: string, contentType: string): Promise<Response>;
  /** A URL that works without a token for `ttl` seconds; `base` is the API's public origin as the caller sees it. */
  url(key: string, ttl: number, base: string): string;
  /** Removes the object; a missing one is not an error. */
  delete(key: string): Promise<void>;
}

function s3(): Storage {
  const client = new Bun.S3Client({
    bucket: process.env.S3_BUCKET!, endpoint: process.env.S3_ENDPOINT, region: process.env.S3_REGION ?? "auto",
    accessKeyId: process.env.S3_ACCESS_KEY_ID, secretAccessKey: process.env.S3_SECRET_ACCESS_KEY,
  });
  return {
    async put(key, body, contentType) {
      const blob = body instanceof Blob ? body : await new Response(body).blob();
      await client.write(key, blob, { type: contentType });
      return blob.size;
    },
    async serve(key) {
      return Response.redirect(client.presign(key, { expiresIn: 300 }), 302);
    },
    url: (key, ttl) => client.presign(key, { expiresIn: ttl }),
    delete: (key) => client.delete(key),
  };
}

function disk(root: string): Storage {
  const path = (key: string) => {
    const p = resolve(root, key);
    if (!p.startsWith(resolve(root) + "/")) throw new Error(`bad key: ${key}`);
    return p;
  };
  return {
    async put(key, body) {
      await mkdir(dirname(path(key)), { recursive: true });
      return Bun.write(path(key), body instanceof Blob ? body : await new Response(body).blob());
    },
    async serve(key, contentType) {
      const f = Bun.file(path(key));
      return (await f.exists()) ? new Response(f, { headers: { "content-type": contentType } }) : new Response(null, { status: 404 });
    },
    async delete(key) {
      await rm(path(key), { force: true });
    },
    url(key, ttl, base) {
      const exp = Math.floor(Date.now() / 1000) + ttl;
      return `${base}/files/${key}?exp=${exp}&sig=${sign(key, exp)}`;
    },
  };
}

export const storage: Storage = process.env.S3_BUCKET ? s3() : disk(process.env.STORAGE_DIR ?? join(process.cwd(), "data", "storage"));

const CHUNK = 4 * 1024 * 1024;

/** GET /files/<key>?exp&sig — the disk store's signed URLs, with byte ranges. */
export async function files(req: Request): Promise<Response> {
  if (process.env.S3_BUCKET) return new Response(null, { status: 404 });
  const u = new URL(req.url);
  const key = decodeURIComponent(u.pathname.replace(/^\/files\//, ""));
  const exp = Number(u.searchParams.get("exp"));
  const sig = Buffer.from(u.searchParams.get("sig") ?? "");
  const want = Buffer.from(sign(key, exp));
  if (!(exp > Date.now() / 1000) || sig.length !== want.length || !timingSafeEqual(sig, want)) return new Response("expired or invalid link", { status: 403 });
  const root = process.env.STORAGE_DIR ?? join(process.cwd(), "data", "storage");
  const p = resolve(root, key);
  if (!p.startsWith(resolve(root) + "/")) return new Response(null, { status: 400 });
  const f = Bun.file(p);
  if (!(await f.exists())) return new Response(null, { status: 404 });
  const type = key.endsWith(".m4a") ? "audio/mp4" : key.endsWith(".ogg") ? "audio/ogg" : key.endsWith(".flac") ? "audio/flac" : "application/octet-stream";
  const headers = { "content-type": type, "accept-ranges": "bytes", "cache-control": "private, max-age=300" };
  const range = req.headers.get("range")?.match(/^bytes=(\d*)-(\d*)$/);
  if (!range) return new Response(f, { headers: { ...headers, "content-length": String(f.size) } });
  const start = range[1] ? Number(range[1]) : Math.max(0, f.size - Number(range[2]));
  // At most CHUNK per answer (players ask for the rest), read into memory: Bun ignores the end of a sliced file body.
  const end = Math.min(range[1] && range[2] ? Number(range[2]) : f.size - 1, f.size - 1, start + CHUNK - 1);
  if (start >= f.size || start > end) return new Response(null, { status: 416, headers: { "content-range": `bytes */${f.size}` } });
  return new Response(await f.slice(start, end + 1).arrayBuffer(), { status: 206, headers: { ...headers, "content-range": `bytes ${start}-${end}/${f.size}`, "content-length": String(end - start + 1) } });
}

/** The origin a request came to, behind proxies too (X-Forwarded-Proto/Host), so links work wherever the caller is. */
export function publicOrigin(req: Request): string {
  const u = new URL(req.url);
  const proto = req.headers.get("x-forwarded-proto")?.split(",")[0].trim() ?? u.protocol.replace(":", "");
  const host = req.headers.get("x-forwarded-host")?.split(",")[0].trim() ?? req.headers.get("host") ?? u.host;
  return `${proto}://${host}`;
}
