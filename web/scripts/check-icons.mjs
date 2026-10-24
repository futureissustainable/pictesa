// Fails the build when src/ui/icon-names.ts and the shipped icon font subset disagree.
// Fix with `pnpm icons` (needs Python fonttools and brotli).
import { readFileSync } from "node:fs";

const root = new URL("..", import.meta.url);
const source = readFileSync(new URL("src/ui/icon-names.ts", root), "utf8");
const block = source.slice(source.indexOf("["), source.lastIndexOf("]"));
const wanted = [...new Set([...block.matchAll(/"([a-z0-9_]+)"/g)].map((match) => match[1]))].sort();
const shipped = JSON.parse(readFileSync(new URL("src/assets/fonts/material-symbols.json", root), "utf8")).icons;
const missing = wanted.filter((name) => !shipped.includes(name));
const extra = shipped.filter((name) => !wanted.includes(name));
if (missing.length || extra.length) {
  console.error(`Icon font is out of date. Run \`pnpm icons\`.${missing.length ? `\n  missing: ${missing.join(", ")}` : ""}${extra.length ? `\n  unused: ${extra.join(", ")}` : ""}`);
  process.exit(1);
}
