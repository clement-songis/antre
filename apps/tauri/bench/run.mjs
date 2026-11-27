// Run the bench pages in the Tauri shell (system WebView) and in Playwright's
// Chromium, sampling the engines' process memory from outside.
//
//   node bench/run.mjs [--ui-dir DIR] [--shell PATH] [--pages a,b] [--engines tauri,chromium] [--runs N] [--out DIR]
//
// Results: one JSON per engine/page/run in --out (default bench/results).
import { execFileSync, spawn } from "node:child_process";
import { createReadStream, existsSync, mkdirSync, statSync, writeFileSync, readFileSync } from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const arg = (name, fallback) => {
	const index = process.argv.indexOf(name);
	return index > 0 ? process.argv[index + 1] : fallback;
};
const uiDir = resolve(arg("--ui-dir", join(here, "../../../crates/web/ui")));
const shell = resolve(arg("--shell", join(here, "../target/debug/antre-shell")));
const pages = arg("--pages", "stream,transcript,canvas").split(",");
const engines = arg("--engines", "tauri,chromium").split(",");
const runs = Number(arg("--runs", "3"));
const outDir = resolve(arg("--out", join(here, "results")));
const dist = join(here, "../dist");
mkdirSync(outDir, { recursive: true });

/** Processes as {pid, ppid, rssMb, command}. */
function processes() {
	const text = execFileSync("ps", ["-axo", "pid=,ppid=,rss=,command="], { encoding: "utf8", maxBuffer: 1 << 26 });
	return text
		.split("\n")
		.filter(Boolean)
		.map((line) => {
			const match = line.trim().match(/^(\d+)\s+(\d+)\s+(\d+)\s+(.*)$/);
			return match && { pid: +match[1], ppid: +match[2], rssMb: +match[3] / 1024, command: match[4] };
		})
		.filter(Boolean);
}

/** Track the peak RSS of the processes `select` picks, every 200 ms. */
function sampler(select) {
	const peaks = {};
	const timer = setInterval(() => {
		const groups = select(processes());
		for (const [name, list] of Object.entries(groups)) {
			const total = list.reduce((sum, p) => sum + p.rssMb, 0);
			peaks[name] = Math.max(peaks[name] ?? 0, Math.round(total));
		}
	}, 200);
	return () => {
		clearInterval(timer);
		return peaks;
	};
}

const WEBKIT = /com\.apple\.WebKit\.(WebContent|GPU|Networking)/;

async function runTauri(page, out) {
	const before = new Set(processes().map((p) => p.pid));
	const startedAt = Date.now();
	const child = spawn(shell, ["--bench", page, "--out", out], { stdio: ["ignore", "inherit", "inherit"] });
	const stop = sampler((list) => {
		const fresh = list.filter((p) => !before.has(p.pid));
		const webkit = (kind) => fresh.filter((p) => p.command.includes(`com.apple.WebKit.${kind}`));
		return {
			shell: fresh.filter((p) => p.pid === child.pid),
			webContent: webkit("WebContent"),
			gpu: webkit("GPU"),
			networking: webkit("Networking"),
		};
	});
	const code = await new Promise((resolveExit) => child.on("exit", resolveExit));
	const peaks = stop();
	if (code !== 0 || !existsSync(out)) throw new Error(`shell exited with ${code} for ${page}`);
	return { wallMs: Date.now() - startedAt, peakRssMb: peaks, result: JSON.parse(readFileSync(out, "utf8")) };
}

const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".png": "image/png" };

function serveDist() {
	return new Promise((resolveServer) => {
		const server = createServer((request, response) => {
			const path = join(dist, decodeURIComponent(new URL(request.url, "http://x").pathname));
			if (!path.startsWith(dist) || !existsSync(path) || statSync(path).isDirectory()) {
				response.writeHead(404).end();
				return;
			}
			response.writeHead(200, { "content-type": TYPES[extname(path)] ?? "application/octet-stream" });
			createReadStream(path).pipe(response);
		});
		server.listen(0, "127.0.0.1", () => resolveServer(server));
	});
}

async function runChromium(browser, base, page) {
	// Playwright exposes no browser pid on a plain launch; its Chromium is the
	// only one running from the ms-playwright cache during a run.
	const before = new Set(processes().map((p) => p.pid));
	const ours = (p) => p.command.includes("ms-playwright");
	const startedAt = Date.now();
	const stop = sampler((list) => {
		const mine = list.filter(ours);
		return {
			renderer: mine.filter((p) => p.command.includes("--type=renderer") && !before.has(p.pid)),
			gpu: mine.filter((p) => p.command.includes("--type=gpu-process")),
			browser: mine.filter((p) => !p.command.includes("--type=")),
		};
	});
	const context = await browser.newContext({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 2 });
	const tab = await context.newPage();
	await tab.goto(`${base}/${page}.html`);
	await tab.waitForFunction(() => window.__benchResult, null, { timeout: 600_000, polling: 500 });
	const result = await tab.evaluate(() => window.__benchResult);
	const peaks = stop();
	await context.close();
	return { wallMs: Date.now() - startedAt, peakRssMb: peaks, result };
}

const summary = [];
if (engines.includes("tauri")) {
	for (const page of pages) {
		for (let run = 1; run <= runs; run++) {
			const out = join(outDir, `tauri-${page}-${run}.json`);
			console.error(`tauri ${page} run ${run}`);
			const record = await runTauri(page, `${out}.raw`);
			writeFileSync(out, JSON.stringify(record, null, 2));
			summary.push({ engine: "tauri", page, run, file: out });
		}
	}
}
if (engines.includes("chromium")) {
	const require = createRequire(join(uiDir, "package.json"));
	const { chromium } = require("@playwright/test");
	const server = await serveDist();
	const base = `http://127.0.0.1:${server.address().port}`;
	const browser = await chromium.launch({ headless: false, args: ["--disable-background-timer-throttling"] });
	for (const page of pages) {
		for (let run = 1; run <= runs; run++) {
			console.error(`chromium ${page} run ${run}`);
			const record = await runChromium(browser, base, page);
			const out = join(outDir, `chromium-${page}-${run}.json`);
			writeFileSync(out, JSON.stringify(record, null, 2));
			summary.push({ engine: "chromium", page, run, file: out });
		}
	}
	await browser.close();
	server.close();
}
writeFileSync(join(outDir, "index.json"), JSON.stringify(summary, null, 2));
console.error(`wrote ${summary.length} results to ${outDir}`);
