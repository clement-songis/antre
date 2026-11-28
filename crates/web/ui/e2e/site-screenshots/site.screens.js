// App screenshots for the landing page. The conversation is injected client-side (no LLM),
// so the pictures are deterministic and show the real renderers of the current UI.
const path = require("node:path");
const { expect, test } = require("@playwright/test");
const { navigateAndWait, waitForChatSessionReady, waitForWsConnected, watchPageErrors } = require("../helpers");

const outDir =
	process.env.ANTRE_SITE_SCREENSHOTS_DIR || path.resolve(__dirname, "../../../../../target/site-screenshots");

const SESSIONS = [
	{ key: "demo:telegram", label: "Telegram · Groceries", minutesAgo: 48 },
	{ key: "demo:trip", label: "Lisbon trip plan", minutesAgo: 60 * 20 },
	{ key: "demo:backup", label: "NAS backup check", minutesAgo: 60 * 26 },
	{ key: "demo:invoice", label: "Invoice reminders (cron)", minutesAgo: 60 * 24 * 3 },
	{ key: "demo:pi", label: "Raspberry Pi setup", minutesAgo: 60 * 24 * 9 },
];

const HISTORY = [
	{ role: "user", content: "Pull last week's merged PRs and draft the changelog section.", historyIndex: 0 },
	{
		role: "tool_result",
		tool_call_id: "demo-exec",
		tool_name: "exec",
		arguments: { command: "gh pr list --state merged --search 'merged:>=2026-11-21' --json number,title" },
		success: true,
		result: {
			stdout:
				"#412 Slack replies keep their thread\n#409 Signal voice notes are transcribed\n#405 Faster memory search on large vaults\n… 11 more",
			stderr: "",
			exit_code: 0,
		},
		run_id: "run-demo",
		historyIndex: 1,
	},
	{
		role: "tool_result",
		tool_call_id: "demo-edit",
		tool_name: "Edit",
		arguments: {
			file_path: "CHANGELOG.md",
			old_string: "## [Unreleased]\n",
			new_string:
				"## [Unreleased]\n\n### Channels\n\n- Slack replies keep their thread\n- Signal voice notes are transcribed\n\n### Memory\n\n- Faster search on large vaults\n",
		},
		success: true,
		result: { file_path: "CHANGELOG.md", replacements: 1 },
		run_id: "run-demo",
		historyIndex: 2,
	},
	{
		role: "assistant",
		content:
			"I found **14 merged pull requests** and grouped them by area in `CHANGELOG.md`:\n\n- **Channels**: Slack threads, Signal voice notes\n- **Memory**: faster search on large vaults\n- **Sandbox**: Podman rootless support\n\nWant me to open a pull request with this draft?",
		run_id: "run-demo",
		historyIndex: 3,
	},
];

async function seedSessions(page) {
	await page.evaluate((sessions) => {
		const store = window.__antre_stores.sessionStore;
		for (const session of sessions) {
			const at = Date.now() - session.minutesAgo * 60_000;
			store.upsert({ key: session.key, label: session.label, createdAt: at, updatedAt: at, messageCount: 6 });
		}
	}, SESSIONS);
}

async function showConversation(page) {
	await page.evaluate(() => window.__antre_modules["stores/session-history-cache"]?.clearSessionHistory?.("main"));
	await page.evaluate((history) => {
		const state = window.__antre_modules.state;
		const ws = state.ws;
		const originalSend = ws.send.bind(ws);
		ws.send = (raw) => {
			const parsed = JSON.parse(raw);
			if (parsed?.method !== "sessions.switch") return originalSend(raw);
			ws.send = originalSend;
			const resolve = state.pending?.[parsed.id];
			if (typeof resolve === "function") {
				delete state.pending[parsed.id];
				resolve({
					ok: true,
					payload: {
						entry: { key: "main", label: "Weekly changelog", messageCount: history.length },
						history,
						replying: false,
					},
				});
			}
		};
	}, HISTORY);
	await page.evaluate(() => window.__antre_modules.sessions.switchSession("main"));
	await expect(page.locator("#messages .msg.assistant").filter({ hasText: "14 merged pull requests" })).toHaveCount(1);
	await page.evaluate(() => {
		window.__antre_stores.sessionStore.upsert({
			key: "main",
			label: "Weekly changelog",
			updatedAt: Date.now() - 120_000,
		});
	});
	const group = page.locator("#messages").getByRole("button", { name: /1 command, 1 edit/ });
	if (await group.count()) await group.first().click();
}

/**
 * Lets entrance transitions finish, then hides what only exists because the capture runs on a
 * throwaway gateway: no provider is configured (the model picker says "loading…") and the
 * runtime directory path would leak the runner's home directory.
 */
async function settle(page) {
	await page.evaluate(() => document.fonts.ready);
	await page.waitForTimeout(800);
	await page.evaluate(() => {
		const label = document.getElementById("modelComboLabel");
		if (label) label.textContent = "qwen3-coder · local";
		for (const input of document.querySelectorAll("input")) {
			if (/^(\/private|\/tmp|\/home|\/Users)\//.test(input.value)) input.value = "~/.antre/sandbox/home";
		}
	});
}

async function openChat(page) {
	await navigateAndWait(page, "/chats/main");
	await waitForWsConnected(page);
	await waitForChatSessionReady(page);
	await seedSessions(page);
	await showConversation(page);
	await settle(page);
}

test("chat, desktop", async ({ page }) => {
	const errors = watchPageErrors(page);
	await page.setViewportSize({ width: 1440, height: 900 });
	await openChat(page);
	await page.screenshot({ path: path.join(outDir, "chat-desktop.png"), animations: "disabled" });
	expect(errors).toEqual([]);
});

test("settings, desktop", async ({ page }) => {
	const errors = watchPageErrors(page);
	await page.setViewportSize({ width: 1280, height: 860 });
	await navigateAndWait(page, "/settings/sandboxes");
	await waitForWsConnected(page);
	await expect(page.getByText(/Loading/)).toHaveCount(0, { timeout: 15_000 });
	await settle(page);
	await page.screenshot({ path: path.join(outDir, "settings-desktop.png"), animations: "disabled" });
	expect(errors).toEqual([]);
});

test("chat, phone", async ({ page }) => {
	const errors = watchPageErrors(page);
	await page.setViewportSize({ width: 390, height: 844 });
	await openChat(page);
	await page.screenshot({ path: path.join(outDir, "chat-phone.png"), animations: "disabled" });
	expect(errors).toEqual([]);
});
