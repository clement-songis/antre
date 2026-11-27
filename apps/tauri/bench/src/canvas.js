// A heavy generated design in a sandboxed `srcdoc` iframe, the way canvas,
// artifacts and MCP Apps are shown: `sandbox="allow-scripts"` without
// `allow-same-origin`, so the design runs in an opaque origin and talks to
// the host only through postMessage.

import { heavyDesign } from "./fixtures.js";
import { environment, FrameMonitor, jsHeapMb, measureRefresh, report, round, setStatus, sleep, summarize } from "./metrics.js";

const host = document.getElementById("canvas-host");
let frame = null;

function waitFor(cmd) {
	return new Promise((resolve) => {
		const listener = (event) => {
			if (event.source !== frame?.contentWindow) return;
			if (event.data?.type === "bench" && event.data.cmd === cmd) {
				removeEventListener("message", listener);
				resolve(event.data);
			}
		};
		addEventListener("message", listener);
	});
}

const LIGHT = `<!doctype html><html><body style="margin:0;height:4000px;background:linear-gradient(#eef,#fee)"><h1>light</h1><script>
const frames=[];function tick(t){frames.push(t);requestAnimationFrame(tick)}requestAnimationFrame(tick);
addEventListener("message",async e=>{const m=e.data;if(!m||m.type!=="bench"||m.cmd!=="idle")return;const a=performance.now();await new Promise(r=>setTimeout(r,m.ms));const ts=frames.filter(t=>t>=a);const d=ts[ts.length-1]-ts[0];parent.postMessage({type:"bench",cmd:"idle",stats:{fps:Math.round((ts.length-1)*10000/d)/10}},"*")});
parent.postMessage({type:"bench",cmd:"ready"},"*");<\/script></body></html>`;

/** Mount a fresh iframe and time the design's first script run and load. */
async function mount(html, sandbox = "allow-scripts") {
	frame?.remove();
	frame = document.createElement("iframe");
	frame.setAttribute("sandbox", sandbox);
	frame.title = "design";
	const start = performance.now();
	const loaded = new Promise((resolve) => frame.addEventListener("load", resolve, { once: true }));
	const ready = waitFor("ready");
	frame.srcdoc = html;
	host.appendChild(frame);
	await ready;
	const readyMs = performance.now() - start;
	await loaded;
	return { readyMs, loadMs: performance.now() - start };
}

function ask(cmd, extra) {
	const answer = waitFor(cmd);
	frame.contentWindow.postMessage({ type: "bench", cmd, ...extra }, "*");
	return answer;
}

async function main() {
	await sleep(500);
	const refreshMs = await measureRefresh();
	const html = heavyDesign();
	const heapStart = jsHeapMb();

	setStatus("first mount");
	const first = await mount(html);
	await sleep(500);

	setStatus("idle animations");
	const hostMonitor = new FrameMonitor(refreshMs);
	hostMonitor.start();
	const idle = await ask("idle", { ms: 3000, refresh: refreshMs });
	const hostIdle = hostMonitor.stop();

	setStatus("scroll inside the design");
	hostMonitor.start();
	const scroll = await ask("scroll", { ms: 4000, refresh: refreshMs });
	const hostScroll = hostMonitor.stop();

	setStatus("remounts");
	const remounts = [];
	for (let i = 0; i < 5; i++) {
		remounts.push((await mount(heavyDesign(100 + i))).loadMs);
		await sleep(200);
	}

	// Is a slow iframe the content's cost, or the engine throttling a
	// cross-origin (opaque-origin) frame the user has not interacted with?
	setStatus("variants");
	const variants = {};
	await mount(LIGHT);
	await sleep(300);
	variants.lightSandboxedFps = (await ask("idle", { ms: 2000, refresh: refreshMs })).stats.fps;
	await mount(html, "allow-scripts allow-same-origin");
	await sleep(300);
	variants.heavySameOriginFps = (await ask("idle", { ms: 3000, refresh: refreshMs })).stats.fps;

	await report({
		bench: "canvas",
		variants,
		env: environment(refreshMs),
		srcdocBytes: new Blob([html]).size,
		elements: scroll.elements,
		designScrollHeightPx: scroll.scrollHeight,
		firstMount: { readyMs: round(first.readyMs), loadMs: round(first.loadMs) },
		idle3s: { iframe: idle.stats, host: hostIdle },
		scroll4s: { iframe: scroll.stats, host: hostScroll },
		remountLoadMs: summarize(remounts),
		jsHeapMb: { start: heapStart, end: jsHeapMb() },
	});
}

main().catch((error) => report({ bench: "canvas", error: String(error?.stack || error) }));
