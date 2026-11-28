// Captures the app screenshots shown on https://antre-ai.github.io (see docs/README.md).
// Not a test suite: run it with `npx playwright test -c playwright.site-screenshots.config.cjs`.
const { defineConfig } = require("@playwright/test");
const path = require("node:path");

const repoRoot = path.resolve(__dirname, "../../..");
const port = process.env.ANTRE_E2E_PORT || "18431";
const baseURL = process.env.ANTRE_SITE_SCREENSHOTS_URL || `http://127.0.0.1:${port}`;

module.exports = defineConfig({
	testDir: "./e2e/site-screenshots",
	testMatch: /.*\.screens\.js$/,
	fullyParallel: false,
	workers: 1,
	retries: 0,
	timeout: 60_000,
	reporter: "list",
	use: {
		baseURL,
		locale: "en-US",
		timezoneId: "Europe/Paris",
		colorScheme: "dark",
		deviceScaleFactor: 2,
	},
	webServer: process.env.ANTRE_SITE_SCREENSHOTS_URL
		? undefined
		: {
				command: path.join(__dirname, "e2e/start-gateway.sh"),
				url: `${baseURL}/health`,
				reuseExistingServer: false,
				timeout: 240_000,
				env: {
					ANTRE_E2E_PORT: port,
					ANTRE_E2E_RUNTIME_DIR: path.join(repoRoot, "target/site-screenshots-runtime"),
				},
			},
});
