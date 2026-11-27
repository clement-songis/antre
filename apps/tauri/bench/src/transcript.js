// Opening a session with ~2,000 messages: render all of them through the
// chat's Markdown path, highlight code, then scroll the whole log.

import { highlightCodeBlocks } from "../../../../crates/web/ui/src/code-highlight.ts";
import { transcript } from "./fixtures.js";
import {
	environment,
	FrameMonitor,
	jsHeapMb,
	measureRefresh,
	nextFrame,
	report,
	round,
	setStatus,
	sleep,
	summarize,
} from "./metrics.js";
import { renderMarkdown } from "./render.js";

const COUNT = 2000;
const box = document.getElementById("messages");

async function initialRender(messages) {
	const start = performance.now();
	const html = messages.map((message) => renderMarkdown(message.text));
	const parsed = performance.now();
	const fragment = document.createDocumentFragment();
	messages.forEach((message, i) => {
		const el = document.createElement("div");
		el.className = `msg ${message.role}`;
		el.insertAdjacentHTML("afterbegin", html[i]);
		fragment.appendChild(el);
	});
	box.appendChild(fragment);
	box.scrollTop = box.scrollHeight;
	void box.scrollHeight;
	const laidOut = performance.now();
	await nextFrame();
	await nextFrame();
	const painted = performance.now();
	return {
		markdownMs: round(parsed - start),
		domAndLayoutMs: round(laidOut - parsed),
		firstPaintMs: round(painted - start),
	};
}

async function scrollThrough(refreshMs, durationMs) {
	box.scrollTop = 0;
	await nextFrame();
	const max = box.scrollHeight - box.clientHeight;
	const monitor = new FrameMonitor(refreshMs);
	monitor.start();
	await new Promise((resolve) => {
		const start = performance.now();
		const step = (t) => {
			const progress = Math.min(1, (t - start) / durationMs);
			box.scrollTop = max * progress;
			if (progress < 1) requestAnimationFrame(step);
			else resolve();
		};
		requestAnimationFrame(step);
	});
	return monitor.stop();
}

async function randomJumps(count) {
	const max = box.scrollHeight - box.clientHeight;
	const times = [];
	for (let i = 0; i < count; i++) {
		const before = performance.now();
		box.scrollTop = (max * ((i * 7919) % count)) / count;
		await nextFrame();
		await nextFrame();
		times.push(performance.now() - before);
	}
	return summarize(times);
}

async function main() {
	await sleep(500);
	const refreshMs = await measureRefresh();
	const messages = transcript(COUNT);
	const chars = messages.reduce((sum, message) => sum + message.text.length, 0);
	const heapStart = jsHeapMb();
	setStatus("render");
	const render = await initialRender(messages);
	await sleep(300);
	setStatus("highlight");
	const before = performance.now();
	await highlightCodeBlocks(box);
	const highlightMs = performance.now() - before;
	await sleep(300);
	setStatus("scroll");
	const scroll = await scrollThrough(refreshMs, 8000);
	setStatus("jumps");
	const jumps = await randomJumps(40);
	await report({
		bench: "transcript",
		env: environment(refreshMs),
		messages: COUNT,
		chars,
		domNodes: box.getElementsByTagName("*").length,
		scrollHeightPx: box.scrollHeight,
		render,
		highlight: { ms: round(highlightMs), codeBlocks: box.querySelectorAll("pre code").length },
		scroll8s: scroll,
		jumpToPositionMs: jumps,
		jsHeapMb: { start: heapStart, end: jsHeapMb() },
	});
}

main().catch((error) => report({ bench: "transcript", error: String(error?.stack || error) }));
