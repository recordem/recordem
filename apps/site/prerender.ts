// Writes the server-rendered page into dist/index.html, in place of <!--app-->.
import { rmSync } from "node:fs";

const { render } = await import("./.ssr/entry-server.js");
const file = Bun.file(new URL("./dist/index.html", import.meta.url));
await Bun.write(file, (await file.text()).replace("<!--app-->", render()));
rmSync(new URL("./.ssr", import.meta.url), { recursive: true });
console.log("prerendered dist/index.html");
