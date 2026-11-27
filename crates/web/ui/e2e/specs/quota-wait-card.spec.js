const { expect, test } = require("../base-test");
const { navigateAndWait, waitForChatSessionReady, waitForWsConnected, watchPageErrors } = require("../helpers");

test.describe("quota wait card", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
	});

	test("shows the reset countdown and cancels the parked run", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.evaluate(() => {
			var ui = window.__moltis_modules["chat-ui"];
			ui.chatAddErrorCard({
				icon: "⏳",
				title: "Waiting for quota",
				detail: "The 5-hour zai-code quota is used up. Resuming automatically at 18:30 CET.",
				provider: "zai-code",
				resetsAt: Date.now() + 2 * 60 * 60 * 1000,
			});
			ui.addQuotaWaitCancelButton("main");
		});

		const card = page.locator(".error-card").last();
		await expect(card).toContainText("Waiting for quota");
		await expect(card).toContainText("Resuming automatically at 18:30 CET.");
		await expect(card.locator(".error-countdown")).not.toBeEmpty();
		await expect(card).toHaveAttribute("data-quota-wait-session", "main");

		const cancel = card.getByRole("button", { name: "Cancel", exact: true });
		await expect(cancel).toBeVisible();
		await cancel.click();
		await expect(cancel).toBeDisabled();

		expect(pageErrors).toEqual([]);
	});
});
