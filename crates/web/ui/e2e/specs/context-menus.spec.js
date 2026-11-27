const { expect, test } = require("../base-test");
const {
	createSession,
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

function activeSessionKey(page) {
	return page.evaluate(() => window.__moltis_stores?.sessionStore?.activeSessionKey?.value || "");
}

function sessionRow(page, key) {
	return page.locator(`#sessionList .session-item[data-session-key="${key}"]`);
}

async function chatEvent(page, payload) {
	await expectRpcOk(page, "system-event", { event: "chat", payload: { sessionKey: "main", ...payload } });
}

test.describe("Context menus", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
	});

	test("right-clicking a session row opens its menu; pin moves it first", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await createSession(page);
		const key = await activeSessionKey(page);
		const row = sessionRow(page, key);
		await expect(row).toBeVisible();
		await row.click({ button: "right" });
		const menu = page.getByRole("menu");
		await expect(menu).toBeVisible();
		await expect(menu.getByRole("menuitem", { name: "Rename", exact: true })).toBeEnabled();
		await expect(menu.getByRole("menuitem", { name: "Delete", exact: true })).toBeVisible();
		await menu.getByRole("menuitem", { name: "Pin", exact: true }).click();
		await expect(menu).toHaveCount(0);
		await expect(page.locator("#sessionList .session-item").first()).toHaveAttribute("data-session-key", key);
		await row.click({ button: "right" });
		await page.getByRole("menuitem", { name: "Unpin", exact: true }).click();
		expect(pageErrors).toEqual([]);
	});

	test("the row's … button works from the keyboard", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const button = sessionRow(page, "main").getByRole("button", { name: /^Actions for / });
		await button.focus();
		await page.keyboard.press("Enter");
		const menu = page.getByRole("menu");
		await expect(menu).toBeVisible();
		// "Rename" is disabled for the main session, so focus starts on "Pin".
		await expect(menu.getByRole("menuitem", { name: "Pin", exact: true })).toBeFocused();
		await page.keyboard.press("ArrowDown");
		await expect(menu.getByRole("menuitem", { name: "Move to project\u2026", exact: true })).toBeFocused();
		await page.keyboard.press("Escape");
		await expect(menu).toHaveCount(0);
		await expect(button).toBeFocused();
		expect(pageErrors).toEqual([]);
	});

	test("rename from the menu edits the name in place", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await createSession(page);
		const key = await activeSessionKey(page);
		const row = sessionRow(page, key);
		await row.click({ button: "right" });
		await page.getByRole("menuitem", { name: "Rename", exact: true }).click();
		const input = row.getByRole("textbox", { name: "Session name" });
		await expect(input).toBeFocused();
		await input.fill("Renamed from menu");
		await input.press("Enter");
		await expect(row.locator("[data-label-text]")).toHaveText("Renamed from menu");
		expect(pageErrors).toEqual([]);
	});

	test("delete hides the session at once and Undo brings it back", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await createSession(page);
		const key = await activeSessionKey(page);
		const row = sessionRow(page, key);
		await row.click({ button: "right" });
		await page.getByRole("menuitem", { name: "Delete", exact: true }).click();
		await expect(row).toHaveCount(0);
		await page.getByRole("button", { name: "Undo", exact: true }).click();
		await expect(row).toBeVisible();
		const listed = await page.evaluate(async (k) => {
			const res = await window.__moltis_modules.helpers.sendRpc("sessions.list", {});
			return JSON.stringify(res?.payload || "").includes(k);
		}, key);
		expect(listed).toBe(true);
		expect(pageErrors).toEqual([]);
	});

	test("replies and tool cards have their own menus", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await expectRpcOk(page, "chat.clear", {});
		await chatEvent(page, {
			state: "tool_call_start",
			runId: "run-menus",
			toolCallId: "m1",
			toolName: "exec",
			arguments: { command: "ls -la" },
		});
		await chatEvent(page, {
			state: "tool_call_end",
			runId: "run-menus",
			toolCallId: "m1",
			toolName: "exec",
			success: true,
			result: { stdout: "total 0", stderr: "", exit_code: 0 },
		});
		await chatEvent(page, { state: "final", runId: "run-menus", text: "Listed.", messageIndex: 993001 });

		await page.locator("#messages .msg.assistant").filter({ hasText: "Listed." }).click({ button: "right" });
		const menu = page.getByRole("menu", { name: "Reply" });
		await expect(menu.getByRole("menuitem", { name: "Copy as Markdown", exact: true })).toBeVisible();
		await expect(menu.getByRole("menuitem", { name: "Regenerate", exact: true })).toBeEnabled();
		await page.keyboard.press("Escape");

		await page.locator("#tool-run-menus-m1").getByRole("button", { name: "Tool call actions", exact: true }).click();
		await expect(page.getByRole("menuitem", { name: "Copy command", exact: true })).toBeVisible();
		await expect(page.getByRole("menuitem", { name: "Copy output", exact: true })).toBeEnabled();
		await page.keyboard.press("Escape");
		expect(pageErrors).toEqual([]);
	});

	test("the chat title menu holds the session and page actions", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.getByRole("button", { name: "Session actions", exact: true }).click();
		const menu = page.getByRole("menu");
		await expect(menu.getByRole("menuitem", { name: "Save as Markdown", exact: true })).toBeVisible();
		await menu.getByRole("menuitem", { name: "Debug context", exact: true }).click();
		await expect(page.locator("#debugModal")).toBeVisible();
		await page.locator("#debugModalCloseBtn").click();
		expect(pageErrors).toEqual([]);
	});

	test("the sidebar account menu switches the theme", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.getByRole("button", { name: "Settings and account", exact: true }).click();
		await page.getByRole("menuitem", { name: "Dark theme", exact: true }).click();
		await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
		await page.getByRole("button", { name: "Settings and account", exact: true }).click();
		await page.getByRole("menuitem", { name: "System theme", exact: true }).click();
		expect(pageErrors).toEqual([]);
	});
});

test.describe("Context menus on touch", () => {
	test.use({ viewport: { width: 390, height: 844 }, hasTouch: true, isMobile: true });

	test("a long-press on a message opens a bottom sheet", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
		await chatEvent(page, { state: "final", runId: "run-touch", text: "Hold me.", messageIndex: 993101 });
		const reply = page.locator("#messages .msg.assistant").filter({ hasText: "Hold me." });
		await expect(reply).toBeVisible();
		await reply.evaluate((el) => {
			const rect = el.getBoundingClientRect();
			const touch = new Touch({
				identifier: 1,
				target: el,
				clientX: rect.left + 10,
				clientY: rect.top + 10,
			});
			el.dispatchEvent(new TouchEvent("touchstart", { touches: [touch], changedTouches: [touch], bubbles: true }));
		});
		const sheet = page.locator(".context-menu.context-sheet");
		await expect(sheet).toBeVisible();
		await reply.evaluate((el) => {
			el.dispatchEvent(
				new TouchEvent("touchend", { touches: [], changedTouches: [], bubbles: true, cancelable: true }),
			);
		});
		await expect(sheet.getByRole("menuitem", { name: "Copy as Markdown", exact: true })).toBeVisible();
		await sheet.getByRole("button", { name: "Cancel", exact: true }).click();
		await expect(sheet).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});
});
