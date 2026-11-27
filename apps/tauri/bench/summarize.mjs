// Print Markdown tables (median over runs) from bench/results.
//   node bench/summarize.mjs [DIR]
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const dir = resolve(process.argv[2] ?? join(dirname(fileURLToPath(import.meta.url)), "results"));
const index = JSON.parse(readFileSync(join(dir, "index.json"), "utf8"));
const records = index.map((entry) => ({ ...entry, data: JSON.parse(readFileSync(join(dir, entry.file.split("/").at(-1)), "utf8")) }));
const ENGINES = [
	["tauri", "WKWebView (Tauri)"],
	["chromium", "Chromium 145"],
];

function median(values) {
	const sorted = values.filter((v) => typeof v === "number").sort((a, b) => a - b);
	if (!sorted.length) return "–";
	const mid = Math.floor(sorted.length / 2);
	const value = sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
	return Math.round(value * 100) / 100;
}

function table(page, rows) {
	const runs = (engine) => records.filter((r) => r.engine === engine && r.page === page).map((r) => r.data);
	console.log(`\n### ${page}\n`);
	console.log(`| Metric | ${ENGINES.map(([, label]) => label).join(" | ")} |`);
	console.log(`|---|${ENGINES.map(() => "---:").join("|")}|`);
	for (const [label, pick] of rows) {
		const cells = ENGINES.map(([engine]) => median(runs(engine).map((d) => {
			try {
				return pick(d);
			} catch {
				return undefined;
			}
		})));
		console.log(`| ${label} | ${cells.join(" | ")} |`);
	}
	const counts = ENGINES.map(([engine]) => runs(engine).length).join(" / ");
	console.log(`\nRuns: ${counts}.`);
}

const env = [
	["Refresh rate seen by rAF (Hz)", (d) => d.result.env.refreshHz],
];
const memory = (d) => d.peakRssMb.webContent ?? d.peakRssMb.renderer;
const gpu = (d) => d.peakRssMb.gpu;

table("stream", [
	...env,
	["Flat out: max tokens/s (render + layout per delta)", (d) => d.result.flatOut.maxTokensPerSec],
	["Flat out: per-delta ms, first 10% of reply", (d) => d.result.flatOut.firstTenthMeanMs],
	["Flat out: per-delta ms, last 10% of reply", (d) => d.result.flatOut.lastTenthMeanMs],
	["Flat out: per-delta ms p95", (d) => d.result.flatOut.perDeltaMs.p95],
	["100 tok/s: rendered tok/s", (d) => d.result.paced[0].renderedTokensPerSec],
	["100 tok/s: fps", (d) => d.result.paced[0].frames.fps],
	["100 tok/s: dropped frames (%)", (d) => d.result.paced[0].frames.droppedPct],
	["100 tok/s: worst frame (ms)", (d) => d.result.paced[0].frames.worstFrameMs],
	["400 tok/s: rendered tok/s", (d) => d.result.paced[1].renderedTokensPerSec],
	["400 tok/s: fps", (d) => d.result.paced[1].frames.fps],
	["400 tok/s: dropped frames (%)", (d) => d.result.paced[1].frames.droppedPct],
	["400 tok/s: worst frame (ms)", (d) => d.result.paced[1].frames.worstFrameMs],
	["400 tok/s: handler p95 (ms)", (d) => d.result.paced[1].handlerMs.p95],
	["Shiki highlight after stream (ms, incl. init)", (d) => d.result.finalHighlightMs],
	["Peak page process RSS (MB)", memory],
	["Peak GPU process RSS (MB)", gpu],
]);

table("transcript", [
	...env,
	["Markdown for 2,000 messages (ms)", (d) => d.result.render.markdownMs],
	["DOM insert + layout (ms)", (d) => d.result.render.domAndLayoutMs],
	["Time to first paint (ms)", (d) => d.result.render.firstPaintMs],
	["Shiki highlight of all code blocks (ms)", (d) => d.result.highlight.ms],
	["Code blocks", (d) => d.result.highlight.codeBlocks],
	["DOM nodes", (d) => d.result.domNodes],
	["Scroll 8 s top→bottom: fps", (d) => d.result.scroll8s.fps],
	["Scroll: dropped frames (%)", (d) => d.result.scroll8s.droppedPct],
	["Scroll: frames > 50 ms", (d) => d.result.scroll8s.framesOver50ms],
	["Scroll: worst frame (ms)", (d) => d.result.scroll8s.worstFrameMs],
	["Jump to position, p95 (ms to 2nd frame)", (d) => d.result.jumpToPositionMs.p95],
	["Peak page process RSS (MB)", memory],
	["Peak GPU process RSS (MB)", gpu],
]);

table("canvas", [
	...env,
	["srcdoc size (KB)", (d) => d.result.srcdocBytes / 1024],
	["Elements in the design", (d) => d.result.elements],
	["First mount: design script runs (ms)", (d) => d.result.firstMount.readyMs],
	["First mount: load event (ms)", (d) => d.result.firstMount.loadMs],
	["Remount load, mean of 5 (ms)", (d) => d.result.remountLoadMs.mean],
	["Idle animations 3 s: iframe fps", (d) => d.result.idle3s.iframe.fps],
	["Idle: iframe dropped (%)", (d) => d.result.idle3s.iframe.droppedPct],
	["Scroll inside design 4 s: iframe fps", (d) => d.result.scroll4s.iframe.fps],
	["Scroll: iframe dropped (%)", (d) => d.result.scroll4s.iframe.droppedPct],
	["Scroll: iframe worst frame (ms)", (d) => d.result.scroll4s.iframe.worstFrameMs],
	["Scroll: host page fps", (d) => d.result.scroll4s.host.fps],
	["Variant: light page, sandboxed: iframe fps", (d) => d.result.variants.lightSandboxedFps],
	["Variant: heavy design, same-origin: iframe fps", (d) => d.result.variants.heavySameOriginFps],
	["Peak page process RSS (MB)", memory],
	["Peak GPU process RSS (MB)", gpu],
]);
