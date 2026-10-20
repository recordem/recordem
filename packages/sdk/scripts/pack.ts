// npm pack/publish: in the monorepo every app imports the TypeScript source (exports → src/index.ts, no build step);
// the published package is built JavaScript and types. `prepack` builds dist/ and points package.json at it, keeping
// the monorepo's manifest aside; `postpack` puts it back, whether the pack succeeded or not.
//   bun scripts/pack.ts prepack | postpack
import { $ } from "bun";
import { copyFile, rename, rm } from "node:fs/promises";

const KEPT = ".package.monorepo.json";

if (process.argv[2] === "prepack") {
  await rm("dist", { recursive: true, force: true });
  await $`bun build src/index.ts --outdir dist --target browser --format esm --packages external`;
  await $`bunx tsc -p tsconfig.build.json`;
  await copyFile("src/schema.d.ts", "dist/schema.d.ts");
  const pkg = await Bun.file("package.json").json();
  await rename("package.json", KEPT);
  pkg.exports = { ".": { types: "./dist/index.d.ts", default: "./dist/index.js" } };
  pkg.types = "./dist/index.d.ts";
  await Bun.write("package.json", `${JSON.stringify(pkg, null, 2)}\n`);
} else if (process.argv[2] === "postpack") {
  if (await Bun.file(KEPT).exists()) await rename(KEPT, "package.json");
} else {
  console.error("usage: bun scripts/pack.ts prepack|postpack");
  process.exit(2);
}
