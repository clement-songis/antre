// Frame timing and reporting shared by the bench pages.
//
// Frames are counted with requestAnimationFrame. A frame is "dropped" when
// the gap between two callbacks spans more than one display refresh; the
// refresh interval is measured on an idle page first, since this Mac's
// display may run at 60 or 120 Hz.

export function nextFrame() {
	return new Promise((resolve) => requestAnimationFrame(resolve));
}

export function sleep(ms) {
	return new Promise((resolve) => setTimeout(resolve, ms));
}

export function percentile(values, p) {
	if (values.length === 0) return 0;
	const sorted = [...values].sort((a, b) => a - b);
	const index = Math.min(sorted.length - 1, Math.ceil((p / 100) * sorted.length) - 1);
	return sorted[Math.max(0, index)];
}

export function round(value, digits = 2) {
	const factor = 10 ** digits;
	return Math.round(value * factor) / factor;
}

export function summarize(values) {
	const total = values.reduce((sum, value) => sum + value, 0);
	return {
		count: values.length,
		mean: round(values.length ? total / values.length : 0),
		p50: round(percentile(values, 50)),
		p95: round(percentile(values, 95)),
		p99: round(percentile(values, 99)),
		max: round(values.length ? Math.max(...values) : 0),
		total: round(total),
	};
}

/** Median rAF interval on an idle page: the display refresh period. */
export async function measureRefresh(frames = 90) {
	const times = [];
	for (let i = 0; i < frames; i++) times.push(await nextFrame());
	const intervals = times.slice(1).map((t, i) => t - times[i]);
	return percentile(intervals, 50);
}

export class FrameMonitor {
	constructor(refreshMs) {
		this.refreshMs = refreshMs;
		this.times = [];
		this.running = false;
	}

	start() {
		this.times = [];
		this.running = true;
		// A stopped loop may still have one callback queued; the generation
		// keeps it from running alongside the new loop.
		const generation = (this.generation = (this.generation ?? 0) + 1);
		const tick = (t) => {
			if (!this.running || generation !== this.generation) return;
			this.times.push(t);
			requestAnimationFrame(tick);
		};
		requestAnimationFrame(tick);
	}

	stop() {
		this.running = false;
		const intervals = this.times.slice(1).map((t, i) => t - this.times[i]);
		const durationMs = this.times.length > 1 ? this.times.at(-1) - this.times[0] : 0;
		const expected = durationMs / this.refreshMs;
		const dropped = intervals.reduce(
			(sum, dt) => sum + Math.max(0, Math.round(dt / this.refreshMs) - 1),
			0,
		);
		return {
			durationMs: round(durationMs),
			frames: this.times.length,
			fps: round(durationMs ? ((this.times.length - 1) * 1000) / durationMs : 0, 1),
			droppedFrames: dropped,
			droppedPct: round(expected ? (100 * dropped) / expected : 0, 1),
			framesOver50ms: intervals.filter((dt) => dt > 50).length,
			worstFrameMs: round(intervals.length ? Math.max(...intervals) : 0),
			frameP95Ms: round(percentile(intervals, 95)),
		};
	}
}

export function jsHeapMb() {
	const memory = performance.memory;
	return memory ? round(memory.usedJSHeapSize / 1048576, 1) : null;
}

export function environment(refreshMs) {
	return {
		userAgent: navigator.userAgent,
		engine: window.__TAURI__ ? "tauri-system-webview" : "browser",
		devicePixelRatio: window.devicePixelRatio,
		viewport: `${window.innerWidth}x${window.innerHeight}`,
		refreshMs: round(refreshMs),
		refreshHz: round(1000 / refreshMs, 0),
	};
}

/** Hand the report to the shell (Tauri IPC) or to the Playwright harness. */
export async function report(result) {
	window.__benchResult = result;
	const status = document.getElementById("bench-status");
	if (status) status.textContent = "done";
	document.title = "bench:done";
	if (window.__TAURI__) {
		await window.__TAURI__.core.invoke("bench_report", { report: result });
	}
}

export function setStatus(text) {
	const status = document.getElementById("bench-status");
	if (status) status.textContent = text;
	window.__TAURI__?.core.invoke("bench_log", { line: text });
}

addEventListener("error", (event) => setStatus(`error: ${event.message}`));
addEventListener("unhandledrejection", (event) => setStatus(`rejection: ${event.reason}`));
setStatus(`loaded, tauri=${Boolean(window.__TAURI__)} visibility=${document.visibilityState}`);
document.addEventListener("visibilitychange", () => setStatus(`visibility=${document.visibilityState}`));

/** Deterministic PRNG (mulberry32) so every engine renders the same content. */
export function rng(seed) {
	let state = seed >>> 0;
	return () => {
		state = (state + 0x6d2b79f5) >>> 0;
		let t = state;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}
