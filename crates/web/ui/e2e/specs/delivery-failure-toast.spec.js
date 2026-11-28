const { expect, test } = require("../base-test");
const { navigateAndWait, waitForChatSessionReady, waitForWsConnected, watchPageErrors } = require("../helpers");

function deliveryEvent(state, error) {
	return {
		id: 7,
		sessionKey: "telegram:bot:42",
		runId: "run-1",
		kind: "reply",
		state: state,
		attempts: 8,
		error: error,
		channelType: "telegram",
		preview: "the answer",
		redelivered: false,
	};
}

test.describe("channel delivery failures", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
	});

	test("a reply lost for good shows an error toast", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const shown = await page.evaluate(
			(event) => {
				return window.__moltis_modules["delivery-events"].handleDeliveryEvent(event);
			},
			deliveryEvent("failed", "telegram 502"),
		);
		expect(shown).toBe(true);

		await expect(
			page.getByText("Could not deliver a reply to Telegram: telegram 502. It will be resent when they write again.", {
				exact: true,
			}),
		).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("retries and deliveries stay quiet", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const shown = await page.evaluate(
			(events) => {
				var handler = window.__moltis_modules["delivery-events"].handleDeliveryEvent;
				return events.map((event) => handler(event));
			},
			[deliveryEvent("queued", "telegram 502"), deliveryEvent("delivered", null)],
		);
		expect(shown).toEqual([false, false]);
		await expect(page.getByText(/Could not deliver a reply/)).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});
});
