// Bundle the bench pages into apps/tauri/dist (the shell's frontendDist).
//
//   node bench/build.mjs [--ui-dir DIR]
//
// DIR is a crates/web/ui checkout with node_modules installed (marked,
// shiki, esbuild) and a built style.css next to it in ../src/assets/css;
// defaults to this repository's crates/web/ui.
import { copyFileSync, existsSync, mkdirSync, readdirSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const flag = process.argv.indexOf("--ui-dir");
const uiDir = resolve(flag > 0 ? process.argv[flag + 1] : join(here, "../../../crates/web/ui"));
const out = join(here, "../dist");
const require = createRequire(join(uiDir, "package.json"));
const esbuild = require("esbuild");

mkdirSync(out, { recursive: true });
for (const file of readdirSync(join(here, "static"))) copyFileSync(join(here, "static", file), join(out, file));
const css = join(uiDir, "../src/assets/css/style.css");
if (!existsSync(css)) throw new Error(`missing ${css}: run npm run build:css in ${uiDir}`);
copyFileSync(css, join(out, "style.css"));

await esbuild.build({
	entryPoints: ["stream", "transcript", "canvas"].map((name) => join(here, "src", `${name}.js`)),
	outdir: out,
	bundle: true,
	splitting: true,
	format: "esm",
	minify: true,
	target: "es2022",
	nodePaths: [join(uiDir, "node_modules")],
	logLevel: "info",
});
