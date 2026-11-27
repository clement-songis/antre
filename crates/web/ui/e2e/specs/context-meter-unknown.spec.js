const { expect, test } = require("../base-test");
const { navigateAndWait, waitForChatSessionReady, waitForWsConnected, watchPageErrors } = require("../helpers");

test.describe("context meter with an unknown window", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
	});

	test("token bar shows ? instead of a percentage of a guessed window", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const read = (windowKnown) =>
			page.evaluate((isKnown) => {
				var S = window.__moltis_modules.state;
				var ui = window.__moltis_modules["chat-ui"];
				S.setSessionCurrentContextTokens(64000);
				S.setSessionContextWindow(128000);
				S.setSessionContextWindowKnown(isKnown);
				ui.updateTokenBar();
				var bar = document.getElementById("tokenBar");
				return { text: bar.textContent, title: bar.title };
			}, windowKnown);

		const known = await read(true);
		expect(known.text).toContain("(50%)");

		const unknown = await read(false);
		expect(unknown.text).toContain("(?)");
		expect(unknown.text).not.toContain("%");
		expect(unknown.title).toContain("context window unknown");

		expect(pageErrors).toEqual([]);
	});
});
