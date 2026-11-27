// Streaming a long assistant reply the way the chat does it: every delta
// re-renders the whole accumulated Markdown into the message element, then
// scrolls to the bottom (ws/chat-handlers.ts handleChatDelta).

import { highlightCodeBlocks } from "../../../../crates/web/ui/src/code-highlight.ts";
import { longReply, tokenize } from "./fixtures.js";
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
import { setSafeMarkdownHtml } from "./render.js";

const TOKENS_PER_DELTA = 3;
const PACED_RATES = [100, 400];

const box = document.getElementById("messages");

function deltas() {
	const tokens = tokenize(longReply());
	const list = [];
	for (let i = 0; i < tokens.length; i += TOKENS_PER_DELTA) {
		list.push(tokens.slice(i, i + TOKENS_PER_DELTA).join(""));
	}
	return { tokens: tokens.length, list };
}

function newMessage() {
	box.textContent = "";
	const user = document.createElement("div");
	user.className = "msg user";
	user.textContent = "Write a long technical answer with code.";
	box.appendChild(user);
	const el = document.createElement("div");
	el.className = "msg assistant";
	box.appendChild(el);
	return el;
}

function scrollToBottom() {
	box.scrollTop = box.scrollHeight;
}

/**
 * Deltas arrive at `rate` tokens/s, each handled in its own task like a
 * WebSocket message. When rendering falls behind, messages queue up.
 */
async function paced(list, tokens, rate, refreshMs) {
	const el = newMessage();
	await nextFrame();
	const intervalMs = (TOKENS_PER_DELTA * 1000) / rate;
	const handlerMs = [];
	const monitor = new FrameMonitor(refreshMs);
	let text = "";
	const start = performance.now();
	monitor.start();
	await new Promise((resolve) => {
		list.forEach((delta, i) => {
			setTimeout(() => {
				const before = performance.now();
				text += delta;
				setSafeMarkdownHtml(el, text);
				scrollToBottom();
				handlerMs.push(performance.now() - before);
				if (i % 200 === 0) setStatus(`paced ${rate}: delta ${i}/${list.length} at ${Math.round(performance.now() - start)} ms`);
				if (i === list.length - 1) resolve();
			}, i * intervalMs);
		});
	});
	await nextFrame();
	const end = performance.now();
	const frames = monitor.stop();
	const idealMs = (list.length - 1) * intervalMs;
	return {
		targetTokensPerSec: rate,
		renderedTokensPerSec: round((tokens * 1000) / (end - start), 1),
		lagMs: round(end - start - idealMs),
		handlerMs: summarize(handlerMs),
		frames,
	};
}

/** Every delta rendered back to back, forcing layout each time: the cost ceiling. */
function flatOut(list, tokens) {
	const el = newMessage();
	const costs = [];
	let text = "";
	const start = performance.now();
	for (const delta of list) {
		const before = performance.now();
		text += delta;
		setSafeMarkdownHtml(el, text);
		scrollToBottom();
		void box.scrollHeight;
		costs.push(performance.now() - before);
	}
	const total = performance.now() - start;
	const tenth = Math.max(1, Math.floor(costs.length / 10));
	const mean = (values) => round(values.reduce((a, b) => a + b, 0) / values.length, 3);
	return {
		deltas: list.length,
		totalMs: round(total),
		maxTokensPerSec: round((tokens * 1000) / total, 0),
		firstTenthMeanMs: mean(costs.slice(0, tenth)),
		lastTenthMeanMs: mean(costs.slice(-tenth)),
		perDeltaMs: summarize(costs),
		domNodes: el.getElementsByTagName("*").length,
	};
}

async function main() {
	await sleep(500);
	const refreshMs = await measureRefresh();
	setStatus(`refresh ${refreshMs.toFixed(2)} ms`);
	const { tokens, list } = deltas();
	const heapStart = jsHeapMb();
	setStatus("flat out");
	const flat = flatOut(list, tokens);
	await sleep(300);
	const pacedResults = [];
	for (const rate of PACED_RATES) {
		setStatus(`paced ${rate} tok/s`);
		pacedResults.push(await paced(list, tokens, rate, refreshMs));
		await sleep(300);
	}
	setStatus("highlight");
	const el = box.querySelector(".msg.assistant");
	const before = performance.now();
	await highlightCodeBlocks(el);
	const highlightMs = performance.now() - before;
	await report({
		bench: "stream",
		env: environment(refreshMs),
		reply: { chars: list.join("").length, tokens, deltas: list.length, tokensPerDelta: TOKENS_PER_DELTA },
		flatOut: flat,
		paced: pacedResults,
		finalHighlightMs: round(highlightMs),
		codeBlocks: el.querySelectorAll("pre code").length,
		jsHeapMb: { start: heapStart, end: jsHeapMb() },
	});
}

main().catch((error) => report({ bench: "stream", error: String(error?.stack || error) }));
