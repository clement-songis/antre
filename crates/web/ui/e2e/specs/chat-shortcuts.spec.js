const { expect, test } = require("../base-test");
const {
	createSession,
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

const MOD = process.platform === "darwin" ? "Meta" : "Control";

function activeSessionKey(page) {
	return page.evaluate(() => window.__antre_stores?.sessionStore?.activeSessionKey?.value || "");
}

async function chatEvent(page, payload) {
	await expectRpcOk(page, "system-event", { event: "chat", payload: { sessionKey: "main", ...payload } });
}

test.describe("Chat keyboard shortcuts", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
	});

	test("the help dialog lists the shortcuts", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.locator("body").click({ position: { x: 1, y: 1 } });
		await page.keyboard.press(`${MOD}+Slash`);
		const dialog = page.getByRole("dialog", { name: "Keyboard shortcuts", exact: true });
		await expect(dialog).toBeVisible();
		await expect(dialog.getByText("New chat", { exact: true })).toBeVisible();
		await expect(dialog.getByText("Copy the last reply", { exact: true })).toBeVisible();
		await page.keyboard.press("Escape");
		await expect(dialog).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("new chat, next and previous session", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.keyboard.press(`${MOD}+Shift+KeyO`);
		await expect.poll(() => activeSessionKey(page)).not.toBe("main");
		const created = await activeSessionKey(page);
		await expect(page.locator(`#sessionList .session-item[data-session-key="${created}"]`)).toBeVisible();

		const order = await page.evaluate(() =>
			[...document.querySelectorAll("#sessionList .session-item[data-session-key]")]
				.filter((row) => row.offsetParent !== null)
				.map((row) => row.dataset.sessionKey),
		);
		const index = order.indexOf(created);
		const next = order[(index + 1) % order.length];
		await page.keyboard.press("Alt+Shift+ArrowDown");
		await expect.poll(() => activeSessionKey(page)).toBe(next);
		await page.keyboard.press("Alt+Shift+ArrowUp");
		await expect.poll(() => activeSessionKey(page)).toBe(created);
		expect(pageErrors).toEqual([]);
	});

	test("toggle the session list and focus the composer", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const list = page.locator("#sessionList");
		await expect(list).toBeVisible();
		await page.keyboard.press(`${MOD}+Shift+KeyS`);
		await expect(page.locator("body")).toHaveClass(/sidebar-collapsed/);
		await expect(list).toBeHidden();
		await expect(page.locator("#newSessionBtn")).toBeVisible();
		await page.reload();
		await waitForWsConnected(page);
		await expect(list).toBeHidden();
		await page.keyboard.press(`${MOD}+Shift+KeyS`);
		await expect(list).toBeVisible();

		await page.locator("body").click({ position: { x: 1, y: 1 } });
		await page.keyboard.press("Shift+Escape");
		await expect(page.locator("#chatInput")).toBeFocused();
		expect(pageErrors).toEqual([]);
	});

	test("copy the last reply", async ({ page, context }) => {
		const pageErrors = watchPageErrors(page);
		await context.grantPermissions(["clipboard-read", "clipboard-write"]);
		await expectRpcOk(page, "chat.clear", {});
		await chatEvent(page, { state: "final", runId: "run-copy", text: "Copy **me**.", messageIndex: 994001 });
		await expect(page.locator("#messages .msg.assistant").filter({ hasText: "Copy me." })).toBeVisible();
		await page.locator("body").click({ position: { x: 1, y: 1 } });
		await page.keyboard.press("Alt+Shift+KeyC");
		await expect(page.getByText("Last reply copied", { exact: true })).toBeVisible();
		expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("Copy **me**.");
		expect(pageErrors).toEqual([]);
	});
});

test.describe("Background session notices", () => {
	test("a finished reply elsewhere counts in the title and offers to open it", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await createSession(page);
		const other = await activeSessionKey(page);
		await page.locator('#sessionList .session-item[data-session-key="main"]').click();
		await expect.poll(() => activeSessionKey(page)).toBe("main");
		await expect(page).not.toHaveTitle(/^\(\d+\) /);

		await page.evaluate((key) => {
			window.__antre_stores.sessionStore.getByKey(key).replying.value = true;
		}, other);
		await expectRpcOk(page, "system-event", {
			event: "chat",
			payload: { sessionKey: other, state: "final", runId: "run-bg", text: "Done in the background.", messageIndex: 1 },
		});
		await expect(page).toHaveTitle(/^\(1\) /);
		const open = page.getByRole("button", { name: "Open", exact: true });
		await expect(open).toBeVisible();
		await open.click();
		await expect.poll(() => activeSessionKey(page)).toBe(other);
		await expect(page).not.toHaveTitle(/^\(\d+\) /);
		expect(pageErrors).toEqual([]);
	});
});
