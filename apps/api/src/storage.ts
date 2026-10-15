// Where audio lives. S3-compatible (R2, AWS, Garage…) through Bun's own client when S3_BUCKET is set; otherwise a
// folder on disk, so a self-hosted server needs nothing else to start.
import { mkdir } from "node:fs/promises";
import { dirname, join, resolve } from "node:path";

export interface Storage {
  put(key: string, body: Blob | ReadableStream, contentType: string): Promise<number>;
  /** A response that serves the object: a redirect to a short-lived URL (S3) or the bytes (disk). */
  serve(key: string, contentType: string): Promise<Response>;
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
  };
}

export const storage: Storage = process.env.S3_BUCKET ? s3() : disk(process.env.STORAGE_DIR ?? join(process.cwd(), "data", "storage"));
