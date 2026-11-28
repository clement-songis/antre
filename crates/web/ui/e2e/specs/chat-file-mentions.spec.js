const path = require("node:path");
const { expect, test } = require("../base-test");
const {
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

// The e2e folder itself is the project: its files are known and stable.
const PROJECT_DIR = path.resolve(__dirname, "..");
const PROJECT_ID = "e2e-file-mentions";

test.describe("@file mentions", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		const now = Date.now();
		await expectRpcOk(page, "projects.upsert", {
			id: PROJECT_ID,
			label: "E2E mentions",
			directory: PROJECT_DIR,
			created_at: now,
			updated_at: now,
		});
		await expectRpcOk(page, "sessions.patch", { key: "main", projectId: PROJECT_ID });
		await page.evaluate((id) => {
			const session = window.__antre_stores.sessionStore.getByKey("main");
			if (session) session.projectId = id;
		}, PROJECT_ID);
	});

	test.afterEach(async ({ page }) => {
		await expectRpcOk(page, "sessions.patch", { key: "main", projectId: "" });
		await expectRpcOk(page, "projects.delete", { id: PROJECT_ID });
	});

	test("typing @ lists project files and Enter inserts the path", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const input = page.locator("#chatInput");
		await input.fill("");
		await input.pressSequentially("look at @helpe");
		const menu = page.getByRole("listbox", { name: "Files" });
		await expect(menu).toBeVisible();
		await expect(menu.getByRole("option").first()).toContainText("helpers.js");
		await page.keyboard.press("Enter");
		await expect(input).toHaveValue("look at @helpers.js ");
		await expect(menu).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("Escape closes the menu without changing the text", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const input = page.locator("#chatInput");
		await input.fill("");
		await input.pressSequentially("@base-te");
		const menu = page.getByRole("listbox", { name: "Files" });
		await expect(menu).toBeVisible();
		await page.keyboard.press("Escape");
		await expect(menu).toHaveCount(0);
		await expect(input).toHaveValue("@base-te");
		expect(pageErrors).toEqual([]);
	});
});
