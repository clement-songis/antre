const { expect, test } = require("../base-test");
const { navigateAndWait, waitForWsConnected, watchPageErrors } = require("../helpers");

async function mockVoiceProviders(page) {
	await expect.poll(() => new URL(page.url()).pathname).toBe("/settings/voice");
	await expect(page.getByRole("heading", { name: "Voice", exact: true })).toBeVisible();
	await page.waitForFunction(() => !!document.querySelector('script[type="module"][src*="js/app.js"]'));
	await page.evaluate(async () => {
		const appScript = document.querySelector('script[type="module"][src*="js/app.js"]');
		if (!appScript) throw new Error("app.js script not found");
		const appUrl = new URL(appScript.src, window.location.origin).href;
		const marker = "js/app.js";
		const markerIdx = appUrl.indexOf(marker);
		if (markerIdx < 0) throw new Error("app.js marker not found in script URL");
		const prefix = appUrl.slice(0, markerIdx);
		const state = await import(`${prefix}js/state.js`);
		const wsOpen = typeof WebSocket === "undefined" ? 1 : WebSocket.OPEN;
		window.__voiceSettingsRequests = [];
		const providers = {
			stt: [
				{
					id: "gemini",
					name: "Google Gemini",
					type: "stt",
					category: "cloud",
					description: "Gemini's dedicated transcription model, 85+ languages auto-detected, live through the Live API",
					available: true,
					enabled: true,
					preferred: true,
					keySource: "env",
					keyPlaceholder: "API key",
					keyUrl: "https://aistudio.google.com/apikey",
					keyUrlLabel: "aistudio.google.com",
					hint: "Uses a Google AI Studio key, the same one as the Gemini LLM provider.",
					capabilities: {
						customModel: true,
						modelChoices: ["gemini-3.5-transcribe"],
						realtimeModelChoices: ["gemini-3.5-transcribe-live"],
					},
					settings: {
						model: null,
						realtimeModel: null,
					},
					settingsSummary: null,
				},
			],
			tts: [],
		};
		state.setConnected(false);
		function respond(req) {
			if (req.method === "voice.providers.all") return { ok: true, payload: providers };
			if (req.method === "voice.config.voxtral_requirements") {
				return { ok: true, payload: { os: "macos", arch: "aarch64", compatible: false, reasons: [] } };
			}
			if (req.method === "voice.config.save_settings") return { ok: true, payload: { ok: true } };
			if (req.method === "voice.provider.toggle") return { ok: true, payload: { ok: true } };
			if (req.method === "voice.personas.list") return { ok: true, payload: { personas: [] } };
			return {
				ok: false,
				error: { message: `unexpected rpc in voice settings test: ${req.method}` },
			};
		}

		state.setWs({
			readyState: wsOpen,
			send(raw) {
				const req = JSON.parse(raw || "{}");
				const resolver = state.pending[req.id];
				if (!resolver) return;
				window.__voiceSettingsRequests.push({
					method: req.method,
					params: req.params || {},
				});
				resolver(respond(req));
				delete state.pending[req.id];
			},
		});
		state.setConnected(true);
	});
}

test.describe("Settings > Voice > Gemini", () => {
	test("saves the Gemini live transcription model", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/voice");
		await waitForWsConnected(page);
		await mockVoiceProviders(page);

		const geminiRow = page.locator(".provider-card").filter({ has: page.getByText("Google Gemini", { exact: true }) });
		await expect(geminiRow).toContainText("live through the Live API");
		await geminiRow.getByRole("button", { name: "Configure", exact: true }).click();

		const modal = page.locator(".modal-box").filter({ has: page.getByText("Add Google Gemini", { exact: true }) });
		await expect(modal).toBeVisible();
		const realtimeModel = modal.getByLabel("Realtime model (live transcription)", { exact: true });
		await expect(realtimeModel).toHaveAttribute("placeholder", "gemini-3.5-transcribe-live (default)");
		await realtimeModel.fill("gemini-3.5-transcribe-live");
		await modal.getByRole("button", { name: "Save", exact: true }).click();

		await expect
			.poll(() =>
				page.evaluate(
					() => window.__voiceSettingsRequests.find((req) => req.method === "voice.config.save_settings") || null,
				),
			)
			.not.toBeNull();
		const saveRequest = await page.evaluate(() =>
			window.__voiceSettingsRequests.find((req) => req.method === "voice.config.save_settings"),
		);
		expect(saveRequest.params).toMatchObject({
			provider: "gemini",
			realtimeModel: "gemini-3.5-transcribe-live",
		});
		expect(pageErrors).toEqual([]);
	});
});
