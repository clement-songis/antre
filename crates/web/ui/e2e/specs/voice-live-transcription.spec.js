// E2E tests: live transcription over /api/stt/ws, with fallback to clip
// transcription when the stream is unavailable.

const { expect, test } = require("../base-test");
const { navigateAndWait, waitForWsConnected, watchPageErrors } = require("../helpers");

const UPLOAD_FILENAME = "voice-live-test.webm";

// A microphone that plays a tone, so MediaRecorder and the capture worklet
// both receive real audio without a physical device.
async function fakeMicrophone(page) {
	await page.addInitScript(() => {
		navigator.mediaDevices.getUserMedia = async () => {
			const context = new AudioContext();
			const oscillator = context.createOscillator();
			const destination = context.createMediaStreamDestination();
			oscillator.connect(destination);
			oscillator.start();
			await context.resume().catch(() => undefined);
			return destination.stream;
		};
	});
}

// Answer the voice RPCs locally and record chat.send; everything else goes to
// the real gateway.
async function mockVoiceRpc(page) {
	await page.evaluate(async () => {
		const appScript = document.querySelector('script[type="module"][src*="js/app.js"]');
		if (!appScript) throw new Error("app module script not found");
		const appUrl = new URL(appScript.src, window.location.origin);
		const prefix = appUrl.href.slice(0, appUrl.href.length - "js/app.js".length);
		const state = await import(`${prefix}js/state.js`);
		// Let the page's own voice status check finish first, so its real
		// answer cannot override the mocked one.
		while (Object.keys(state.pending || {}).length > 0) {
			await new Promise((nextFrame) => requestAnimationFrame(nextFrame));
		}
		const ws = state.ws;
		if (!ws) throw new Error("websocket unavailable");
		const realSend = ws.send.bind(ws);
		window.__chatSends = [];

		function resolve(id, payload) {
			const resolver = state.pending?.[id];
			if (typeof resolver !== "function") return;
			delete state.pending[id];
			resolver({ ok: true, payload });
		}

		ws.send = (raw) => {
			const req = JSON.parse(raw || "{}");
			if (req.method === "voice.providers.all") {
				resolve(req.id, {
					stt: [{ id: "whisper", name: "OpenAI Whisper", type: "stt", available: true, enabled: true }],
					tts: [],
				});
				return;
			}
			if (req.method === "stt.status") {
				resolve(req.id, {
					enabled: true,
					configured: true,
					provider: "whisper",
					streaming: { available: true, provider: "whisper", model: "mock-live" },
				});
				return;
			}
			if (req.method === "chat.send") {
				window.__chatSends.push(req.params || {});
				resolve(req.id, { ok: true });
				return;
			}
			realSend(raw);
		};
		window.dispatchEvent(new Event("voice-config-changed"));
	});
}

async function mockUpload(page, transcriptionText) {
	const uploads = [];
	await page.route("**/api/sessions/*/upload*", async (route) => {
		const url = new URL(route.request().url());
		const transcribe = url.searchParams.get("transcribe") === "true";
		uploads.push({ transcribe });
		const body = { ok: true, filename: UPLOAD_FILENAME };
		if (transcribe) body.transcription = { text: transcriptionText };
		await route.fulfill({ status: 200, contentType: "application/json", body: JSON.stringify(body) });
	});
	return uploads;
}

async function openChatWithMic(page) {
	await navigateAndWait(page, "/chats/main");
	await waitForWsConnected(page);
	await mockVoiceRpc(page);
	const mic = page.getByRole("button", { name: "Click to start recording", exact: true });
	await expect(mic).toBeVisible();
	await expect(mic).toBeEnabled();
	return mic;
}

// MediaRecorder needs about a second of audio for a clip the UI will upload.
async function keepRecording(page, ms) {
	await page.waitForFunction(() => document.querySelector("#micBtn.recording") !== null);
	const started = Date.now();
	await expect.poll(() => Date.now() - started, { intervals: [100] }).toBeGreaterThan(ms);
}

function chatSends(page) {
	return page.evaluate(() => window.__chatSends);
}

test.describe("Voice live transcription", () => {
	test("shows partial text while recording and sends the final transcript", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const stream = { audioFrames: 0, stopped: false };
		await page.routeWebSocket("**/api/stt/ws", (ws) => {
			ws.onMessage((message) => {
				if (typeof message !== "string") {
					stream.audioFrames += 1;
					return;
				}
				const parsed = JSON.parse(message);
				if (parsed.type === "start") {
					ws.send(JSON.stringify({ type: "ready", provider: "whisper", model: "mock-live", sampleRate: 16000 }));
					ws.send(JSON.stringify({ type: "partial", segment: "s1", text: "hello from" }));
				} else if (parsed.type === "stop") {
					stream.stopped = true;
					ws.send(JSON.stringify({ type: "final", segment: "s1", text: "hello from live" }));
					ws.send(JSON.stringify({ type: "done", text: "hello from live" }));
				}
			});
		});
		await fakeMicrophone(page);
		const uploads = await mockUpload(page, "unused clip transcript");

		const mic = await openChatWithMic(page);
		await mic.click();
		await expect(page.getByRole("status")).toContainText("hello from");
		await expect.poll(() => stream.audioFrames).toBeGreaterThan(0);
		await keepRecording(page, 1500);

		// The recording button pulses, so Playwright never sees it as stable for a
		// click; Enter goes through the same handler.
		await page.getByRole("button", { name: "Click to stop and send", exact: true }).press("Enter");

		await expect.poll(() => chatSends(page)).toHaveLength(1);
		const [sent] = await chatSends(page);
		expect(sent).toMatchObject({
			text: "hello from live",
			_input_medium: "voice",
			_audio_filename: UPLOAD_FILENAME,
		});
		expect(stream.stopped).toBe(true);
		expect(uploads).toEqual([{ transcribe: false }]);
		const messages = page.getByRole("log", { name: "Chat messages" });
		await expect(messages.getByText("hello from live", { exact: true })).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("falls back to clip transcription when the stream is unavailable", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.routeWebSocket("**/api/stt/ws", (ws) => {
			ws.onMessage((message) => {
				if (typeof message === "string" && JSON.parse(message).type === "start") {
					ws.send(
						JSON.stringify({
							type: "error",
							code: "provider_unavailable",
							message: "provider down",
							fallback: true,
						}),
					);
					ws.close();
				}
			});
		});
		await fakeMicrophone(page);
		const uploads = await mockUpload(page, "hello from clip");

		const mic = await openChatWithMic(page);
		await mic.click();
		const stop = page.getByRole("button", { name: "Click to stop and send", exact: true });
		await expect(stop).toBeVisible();
		await keepRecording(page, 1500);
		await stop.press("Enter");

		await expect.poll(() => chatSends(page)).toHaveLength(1);
		const [sent] = await chatSends(page);
		expect(sent).toMatchObject({ text: "hello from clip", _audio_filename: UPLOAD_FILENAME });
		expect(uploads).toEqual([{ transcribe: true }]);
		expect(pageErrors).toEqual([]);
	});
});
