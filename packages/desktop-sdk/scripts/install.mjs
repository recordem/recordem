// postinstall: downloads the recordem-desktop engine (and its Parakeet helper) for this platform into bin/, from the
// GitHub release named in engine.json, and checks each file against the SHA-256 written there at publish time. The
// checksums ship inside this package, so a tampered release file is refused.
//   RECORDEM_DESKTOP_PATH=<engine>   use a local engine instead (development): nothing is downloaded
//   RECORDEM_DESKTOP_SKIP_DOWNLOAD=1 download nothing (CI, a platform without an engine)
// Never fails the install: an engine that couldn't be fetched is reported, and init() says so later.
import { createHash } from "node:crypto";
import { chmod, mkdir, readFile, rename, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const say = (m) => console.log(`@recordem/desktop-sdk: ${m}`);

async function main() {
  if (process.env.RECORDEM_DESKTOP_PATH) return say(`using ${process.env.RECORDEM_DESKTOP_PATH}`);
  if (process.env.RECORDEM_DESKTOP_SKIP_DOWNLOAD) return say("download skipped");
  const engine = JSON.parse(await readFile(join(root, "engine.json"), "utf8"));
  const platform = `${process.platform}-${process.arch}`;
  const files = engine.files[platform];
  if (!files) return say(`no engine for ${platform} yet (macOS on Apple silicon only)`);
  await mkdir(join(root, "bin"), { recursive: true });
  for (const [name, sha256] of Object.entries(files)) {
    if (!sha256) return say(`engine ${engine.version} has no checksum for ${name}: not published yet, set RECORDEM_DESKTOP_PATH`);
    const dest = join(root, "bin", name);
    try {
      if (createHash("sha256").update(await readFile(dest)).digest("hex") === sha256) continue;
    } catch {}
    const url = engine.url.replace("{version}", engine.version).replace("{file}", `${name}-${platform}`);
    const res = await fetch(url);
    if (!res.ok) throw new Error(`${url}: ${res.status}`);
    const body = Buffer.from(await res.arrayBuffer());
    const got = createHash("sha256").update(body).digest("hex");
    if (got !== sha256) throw new Error(`${name}: checksum ${got}, expected ${sha256}`);
    await writeFile(`${dest}.part`, body);
    await chmod(`${dest}.part`, 0o755);
    await rename(`${dest}.part`, dest);
    say(`${name} ${engine.version} ✓`);
  }
}

main().catch(async (e) => {
  say(`couldn't fetch the engine (${e.message}); set RECORDEM_DESKTOP_PATH, or reinstall`);
  await rm(join(root, "bin", "recordem-desktop.part"), { force: true });
});
