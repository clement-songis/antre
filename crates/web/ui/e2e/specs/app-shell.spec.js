const { expect, test } = require("../base-test");
const { expectPageContentMounted, navigateAndWait, watchPageErrors } = require("../helpers");

test.use({ serviceWorkers: "block" });

test.describe("App shell", () => {
	test("desktop has no top bar and the sidebar collapses to a rail that persists", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.setViewportSize({ width: 1280, height: 860 });
		await navigateAndWait(page, "/");
		await page.evaluate(() => localStorage.removeItem("antre-sidebar-collapsed"));
		await page.reload();
		await expectPageContentMounted(page);

		await expect(page.locator(".app-topbar")).toBeHidden();
		await expect(page.locator(".app-sidebar-brand #titleLink")).toBeVisible();
		await expect(page.locator("#statusDot")).toBeVisible();

		const collapse = page.getByRole("button", { name: "Collapse sidebar" });
		await collapse.click();
		await expect(page.locator("body")).toHaveClass(/sidebar-collapsed/);
		await expect.poll(async () => (await page.locator("#sessionsPanel").boundingBox())?.width ?? 0).toBeLessThan(80);
		await expect(page.locator("#sessionList")).toBeHidden();
		await expect(page.locator("#newSessionBtn")).toBeVisible();

		await page.reload();
		await expectPageContentMounted(page);
		await expect(page.locator("body")).toHaveClass(/sidebar-collapsed/);

		await page.getByRole("button", { name: "Expand sidebar" }).click();
		await expect(page.locator("body")).not.toHaveClass(/sidebar-collapsed/);
		await expect(page.locator("#sessionList")).toBeVisible();

		expect(pageErrors).toEqual([]);
	});

	test("phone top bar opens the drawer and the overlay closes it", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.setViewportSize({ width: 390, height: 844 });
		await navigateAndWait(page, "/");

		await expect(page.locator(".app-topbar")).toBeVisible();
		await expect(page.locator("#mobileNewSessionBtn")).toBeVisible();
		await expect(page.locator("#sessionsPanel")).not.toHaveClass(/open/);

		await page.getByRole("button", { name: "Open sidebar" }).click();
		await expect(page.locator("#sessionsPanel")).toHaveClass(/open/);
		await expect(page.locator("#newSessionBtn")).toBeVisible();
		await expect(page.locator("#themeToggle")).toBeVisible();

		await page.locator("#sessionsOverlay").click({ position: { x: 370, y: 400 } });
		await expect(page.locator("#sessionsPanel")).not.toHaveClass(/open/);

		expect(pageErrors).toEqual([]);
	});

	test("phone settings opens on a full-width section list", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.setViewportSize({ width: 390, height: 844 });
		await navigateAndWait(page, "/settings/profile");

		await page.locator(".settings-mobile-menu-btn").click();
		const memory = page.locator(".settings-nav-item", { hasText: "Memory" });
		await expect(memory).toBeVisible();
		const box = await memory.boundingBox();
		expect(box?.width ?? 0).toBeGreaterThan(300);
		expect(box?.height ?? 0).toBeGreaterThanOrEqual(44);
		await expect(page.locator(".settings-group-label").first()).toBeVisible();

		expect(pageErrors).toEqual([]);
	});

	test("mods settings shows the placeholder and the kinds of mods", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/mods");

		await expect(page.getByRole("heading", { name: "Mods", exact: true })).toBeVisible();
		await expect(page.getByText("No mods installed", { exact: true })).toBeVisible();
		await expect(page.locator("[data-mod-kind]")).toHaveCount(5);
		await expect(page.locator('.settings-nav-item[data-section="mods"]')).toHaveClass(/active/);

		expect(pageErrors).toEqual([]);
	});

	test("logs and network audit pages have a title", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/logs");
		await expect(page.getByRole("heading", { name: "Logs", exact: true })).toBeVisible();
		await navigateAndWait(page, "/settings/network-audit");
		await expect(page.getByRole("heading", { name: "Network Audit", exact: true })).toBeVisible();

		expect(pageErrors).toEqual([]);
	});

	test("phone command palette opens as a bottom sheet", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.setViewportSize({ width: 390, height: 844 });
		await navigateAndWait(page, "/");

		await page.getByRole("button", { name: "Open sidebar" }).click();
		await page.locator("#commandPaletteBtn").click();
		const palette = page.locator(".cmd-palette");
		await expect(palette).toBeVisible();
		await expect(page.locator("#sessionsPanel")).not.toHaveClass(/open/);
		const box = await palette.boundingBox();
		expect(box?.width ?? 0).toBeGreaterThanOrEqual(388);
		expect(Math.round((box?.y ?? 0) + (box?.height ?? 0))).toBeGreaterThanOrEqual(843);
		await page.keyboard.press("Escape");

		expect(pageErrors).toEqual([]);
	});
});
