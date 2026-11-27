const { expect, test } = require("../base-test");
const { navigateAndWait, waitForChatSessionReady, waitForWsConnected, watchPageErrors } = require("../helpers");

/** Set mock models in the browser and freeze the store so bootstrap/WS cannot overwrite. */
async function setMockModels(page, mockModels, mockSelectedId) {
	await page.evaluate(
		async ([models, selectedId]) => {
			var appScript = document.querySelector('script[type="module"][src*="js/app.js"]');
			var appUrl = new URL(appScript.src, window.location.origin);
			var prefix = appUrl.href.slice(0, appUrl.href.length - "js/app.js".length);
			var store = await import(`${prefix}js/stores/model-store.js`);
			for (var i = 0; i < 100 && store.models.value.length === 0; i++) {
				await new Promise((r) => setTimeout(r, 50));
			}
			store.modelStore.fetch = () => Promise.resolve();
			store.modelStore.setAll = () => undefined;
			store.select(selectedId);
			store.setAll(models);
		},
		[mockModels, mockSelectedId],
	);
}

const catalog = (wireId, extra) => ({
	wireId,
	versionLabel: null,
	contextWindow: null,
	maxOutput: null,
	deprecated: false,
	retiresAt: null,
	...extra,
});

test.describe("model picker catalog details", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
	});

	test("shows the wire id under the name, the context size and deprecation", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await setMockModels(
			page,
			[
				{
					id: "zai::glm-4.6-air-0925",
					displayName: "GLM 4.6 Air",
					provider: "zai",
					catalog: catalog("glm-4.6-air-0925", { versionLabel: "0925" }),
				},
				{
					id: "anthropic::claude-opus-5-5",
					displayName: "Claude Opus 5.5",
					provider: "anthropic",
					catalog: catalog("claude-opus-5-5", { contextWindow: 1000000 }),
				},
				{
					id: "openai::gpt-5-mini",
					displayName: "GPT-5 Mini",
					provider: "openai",
					catalog: catalog("gpt-5-mini", {
						contextWindow: 400000,
						deprecated: true,
						retiresAt: "2026-12-11",
					}),
				},
				{
					id: "custom-box::plain",
					displayName: "plain",
					provider: "custom-box",
					catalog: catalog("plain"),
				},
			],
			"anthropic::claude-opus-5-5",
		);

		await page.locator("#modelComboBtn").click();
		const items = page.locator("#modelDropdownList .model-dropdown-item[data-model-id]");
		await expect(items).toHaveCount(4);

		const glm = items.filter({ hasText: "GLM 4.6 Air" });
		await expect(glm.locator(".model-item-wire-id")).toHaveText("glm-4.6-air-0925");
		await expect(glm.locator(".model-item-context")).toHaveText("?");
		await expect(glm.locator(".model-item-context")).toHaveAttribute("title", "context window unknown");

		const opus = items.filter({ hasText: "Claude Opus 5.5" });
		await expect(opus.locator(".model-item-context")).toHaveText("1M");

		const mini = items.filter({ hasText: "GPT-5 Mini" });
		await expect(mini.locator(".model-item-context")).toHaveText("400K");
		await expect(mini.locator(".model-item-deprecated")).toHaveText("deprecated");
		await expect(mini.locator(".model-item-deprecated")).toHaveAttribute("title", "retires 2026-12-11");

		// A name equal to the wire id is not repeated on a second line.
		const plain = items.filter({ hasText: "plain" });
		await expect(plain.locator(".model-item-wire-id")).toHaveCount(0);

		expect(pageErrors).toEqual([]);
	});
});
