// The reader profiles (how a recorder reads each call app's window), served to every recorder: the files of
// apps/recorder/crates/scrape/profiles, or PROFILES_DIR (a mounted ConfigMap, say) to change them with no new build.
import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const DIR = process.env.PROFILES_DIR ?? join(import.meta.dir, "../../recorder/crates/scrape/profiles");

function load() {
  const profiles = readdirSync(DIR).filter((f) => f.endsWith(".json")).sort().map((f) => JSON.parse(readFileSync(join(DIR, f), "utf8")));
  const body = JSON.stringify({ profiles });
  return { body, etag: `"${createHash("sha256").update(body).digest("hex").slice(0, 32)}"` };
}

const doc = load();

export function serveProfiles(req: Request) {
  const headers = { etag: doc.etag, "cache-control": "public, max-age=300" };
  if (req.headers.get("if-none-match") === doc.etag) return new Response(null, { status: 304, headers });
  return new Response(doc.body, { headers: { ...headers, "content-type": "application/json" } });
}
