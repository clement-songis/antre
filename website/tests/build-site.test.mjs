import assert from "node:assert/strict";
import { mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { after, before, describe, test } from "node:test";

import { buildSite, DEFAULT_ORIGIN, fontFaces, renderTemplate, resolveOrigin } from "../scripts/build-site.mjs";

const exists = (file) =>
	stat(file).then(
		() => true,
		() => false,
	);

describe("resolveOrigin", () => {
	test("defaults to the org Pages host without a CNAME", () => {
		assert.deepEqual(resolveOrigin(undefined), { origin: DEFAULT_ORIGIN, cname: null });
		assert.deepEqual(resolveOrigin("  "), { origin: DEFAULT_ORIGIN, cname: null });
	});

	test("accepts a bare custom domain", () => {
		assert.deepEqual(resolveOrigin("Antre.Example"), { origin: "https://antre.example", cname: "antre.example" });
	});

	for (const bad of ["https://antre.example", "antre.example/docs", "a b.example", 'x".example', "localhost", "-a.example"]) {
		test(`rejects ${JSON.stringify(bad)}`, () => {
			assert.throws(() => resolveOrigin(bad), /SITE_DOMAIN/);
		});
	}
});

describe("renderTemplate", () => {
	test("prefers locale strings over globals", () => {
		assert.equal(renderTemplate("{{a}} {{site}}", { a: "x", site: "local" }, { site: "g" }), "x local");
	});

	test("fails on unknown keys", () => {
		assert.throws(() => renderTemplate("{{missing.key}}", {}, {}), /missing\.key/);
	});
});

describe("fontFaces", () => {
	const css = `/* geologica-cyrillic-wght-normal */
@font-face {
  font-family: 'Geologica Variable';
  src: url(./files/geologica-cyrillic-wght-normal.woff2) format('woff2-variations');
}

/* geologica-latin-ext-wght-normal */
@font-face {
  font-family: 'Geologica Variable';
  src: url(./files/geologica-latin-ext-wght-normal.woff2) format('woff2-variations');
}

/* atkinson-hyperlegible-next-latin-wght-italic */
@font-face {
  font-family: 'Atkinson Hyperlegible Next Variable';
  src: url(./files/atkinson-hyperlegible-next-latin-wght-italic.woff2) format('woff2-variations');
}`;

	test("keeps latin subsets, renames the family and rewrites URLs", () => {
		const faces = fontFaces(css, "Geologica");
		assert.deepEqual(
			faces.map((face) => face.file),
			["geologica-latin-ext-wght-normal.woff2", "atkinson-hyperlegible-next-latin-wght-italic.woff2"],
		);
		assert.match(faces[0].css, /font-family: 'Geologica'/);
		assert.match(faces[0].css, /url\(\/fonts\/geologica-latin-ext-wght-normal\.woff2\)/);
	});
});

describe("buildSite", () => {
	let work;
	before(async () => {
		work = await mkdtemp(path.join(tmpdir(), "antre-site-"));
		await mkdir(path.join(work, "docs"), { recursive: true });
		await writeFile(path.join(work, "docs", "index.html"), "<p>docs</p>");
	});
	after(() => rm(work, { recursive: true, force: true }));

	test("builds EN and FR pages, assets and Pages files without a CNAME by default", async () => {
		const outDir = path.join(work, "default");
		await buildSite({ outDir, docsDir: path.join(work, "docs"), domain: "" });

		const en = await readFile(path.join(outDir, "index.html"), "utf8");
		const fr = await readFile(path.join(outDir, "fr", "index.html"), "utf8");
		assert.match(en, /<html lang="en"/);
		assert.match(fr, /<html lang="fr"/);
		assert.doesNotMatch(en + fr, /\{\{/);
		assert.match(en, /curl -fsSL https:\/\/antre-ai\.github\.io\/install\.sh \| sh/);
		assert.match(en, /href="\/docs\/"/);
		assert.match(en, /Content-Security-Policy/);

		for (const file of [".nojekyll", "404.html", "install.sh", "assets/tokens.css", "assets/fonts.css", "llms.txt", ".well-known/antre-install.json", "docs/index.html"]) {
			assert.ok(await exists(path.join(outDir, file)), `${file} missing`);
		}
		assert.equal(await exists(path.join(outDir, "CNAME")), false);
		assert.match(await readFile(path.join(outDir, "assets", "fonts.css"), "utf8"), /font-family: 'Geologica'/);
	});

	test("writes the CNAME and rewrites the origin when SITE_DOMAIN is set", async () => {
		const outDir = path.join(work, "custom");
		await buildSite({ outDir, docsDir: path.join(work, "docs"), domain: "antre.example" });

		assert.equal(await readFile(path.join(outDir, "CNAME"), "utf8"), "antre.example\n");
		const en = await readFile(path.join(outDir, "index.html"), "utf8");
		assert.match(en, /<link rel="canonical" href="https:\/\/antre\.example\/">/);
		const manifest = await readFile(path.join(outDir, ".well-known", "antre-install.json"), "utf8");
		const llms = await readFile(path.join(outDir, "llms.txt"), "utf8");
		assert.doesNotMatch(manifest + llms, /antre-ai\.github\.io/);
		assert.match(manifest, /https:\/\/antre\.example\/install\.sh/);
	});

	test("fails when docs are required but missing", async () => {
		await assert.rejects(
			buildSite({ outDir: path.join(work, "nodocs"), docsDir: path.join(work, "absent"), domain: "", requireDocs: true }),
			/Docs build not found/,
		);
	});
});
