const { expect, test } = require("../base-test");
const { navigateAndWait, watchPageErrors } = require("../helpers");

function readToken(page, name) {
	return page.evaluate(
		(tokenName) => getComputedStyle(document.documentElement).getPropertyValue(`--${tokenName}`).trim(),
		name,
	);
}

test.describe("Antre design tokens", () => {
	test("tokens follow the data-theme attribute", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/");

		await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));
		expect(await readToken(page, "lake")).toBe("#52c3ce");
		expect(await readToken(page, "ground")).toBe("#0d1214");

		await page.evaluate(() => document.documentElement.setAttribute("data-theme", "light"));
		expect(await readToken(page, "lake")).toBe("#0b6d79");
		expect(await readToken(page, "ground")).toBe("#f5f7f7");

		expect(await readToken(page, "corner-l")).toBe("12px");
		expect(await readToken(page, "font-display")).toContain("Geologica");
		expect(pageErrors).toEqual([]);
	});

	test("component classes resolve to tokens", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/");
		await page.evaluate(() => document.documentElement.setAttribute("data-theme", "dark"));

		const background = await page.evaluate(() => {
			const probe = document.createElement("button");
			probe.className = "an-btn an-btn--primary";
			document.body.appendChild(probe);
			const color = getComputedStyle(probe).backgroundColor;
			probe.remove();
			return color;
		});
		expect(background).toBe("rgb(82, 195, 206)");
		expect(pageErrors).toEqual([]);
	});
});
