const { expect, test } = require("../base-test");
const {
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

async function setLocale(page, lng) {
	await page.evaluate(async (locale) => {
		const appScript = document.querySelector('script[type="module"][src*="js/app.js"]');
		const appUrl = new URL(appScript.src, window.location.origin);
		const prefix = appUrl.href.slice(0, appUrl.href.length - "js/app.js".length);
		const i18n = await import(`${prefix}js/i18n.js`);
		await i18n.setLocale(locale);
	}, lng);
	await expect(page.locator("html")).toHaveAttribute("lang", lng);
}

async function broadcast(page, event, payload) {
	await expectRpcOk(page, "system-event", { event, payload });
}

async function chatEvent(page, payload) {
	await broadcast(page, "chat", { sessionKey: "main", ...payload });
}

/** Answer the listed RPCs locally with the given payloads. */
async function mockRpcs(page, table) {
	await page.evaluate((responses) => {
		const state = window.__moltis_modules.state;
		const ws = state.ws;
		if (!window.__rpcOriginalSend) window.__rpcOriginalSend = ws.send.bind(ws);
		ws.send = (raw) => {
			const parsed = JSON.parse(raw);
			const payload = responses[parsed?.method];
			if (payload === undefined) return window.__rpcOriginalSend(raw);
			const resolve = state.pending?.[parsed.id];
			if (typeof resolve === "function") {
				delete state.pending[parsed.id];
				resolve({ ok: true, payload });
			}
		};
	}, table);
}

test.describe("Chat strings follow the locale", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
	});

	test.afterEach(async ({ page }) => {
		await page.evaluate(() => {
			const ws = window.__moltis_modules?.state?.ws;
			if (ws && window.__rpcOriginalSend) ws.send = window.__rpcOriginalSend;
		});
	});

	test("approvals, tool cards and accessibility labels in French", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await setLocale(page, "fr");

		await broadcast(page, "exec.approval.requested", {
			requestId: "req-fr",
			command: "ls",
			sessionKey: "main",
			expiresAtMs: Date.now() + 30_000,
		});
		const card = page.locator('.approval-dock [data-request-id="req-fr"]');
		await expect(card.getByRole("button", { name: "Autoriser une fois", exact: true })).toBeVisible();
		await expect(card.getByRole("button", { name: "Toujours autoriser", exact: true })).toBeVisible();
		await expect(card.locator(".approval-countdown")).toHaveText(/^\d+\s?s$/);
		await broadcast(page, "exec.approval.resolved", { requestId: "req-fr" });
		await expect(card).toHaveCount(0);

		for (const id of ["fr1", "fr2"]) {
			await chatEvent(page, {
				state: "tool_call_start",
				runId: "run-fr",
				toolCallId: id,
				toolName: "exec",
				arguments: { command: `echo ${id}` },
			});
			await chatEvent(page, {
				state: "tool_call_end",
				runId: "run-fr",
				toolCallId: id,
				toolName: "exec",
				success: true,
				result: { stdout: id, stderr: "", exit_code: 0 },
			});
		}
		await expect(page.locator(".tool-group-summary")).toHaveText("2 commandes");
		await page.locator(".tool-group-header").click();
		await expect(page.locator("#tool-run-fr-fr1 .tool-card-verb")).toHaveText("Exécuté");
		await expect(
			page.locator("#tool-run-fr-fr1").getByRole("button", { name: "Actions de l’appel d’outil", exact: true }),
		).toBeVisible();

		await expect(page.getByRole("button", { name: "Actions de la session", exact: true })).toBeVisible();
		await expect(
			page
				.locator('#sessionList .session-item[data-session-key="main"]')
				.getByRole("button", { name: /^Actions pour / }),
		).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("queue tray and rewind dialog in French", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await setLocale(page, "fr");
		await mockRpcs(page, { "sessions.rewind_points": { points: [] }, "chat.send": { queued: true } });
		// Both presses in one task so they stay inside the double-press window.
		await page.locator("#chatInput").evaluate((input) => {
			for (let i = 0; i < 2; i++) {
				input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
			}
		});
		const dialog = page.getByRole("dialog", { name: "Revenir en arrière", exact: true });
		await expect(dialog).toBeVisible();
		await expect(dialog.getByRole("button", { name: "Fermer", exact: true })).toBeVisible();
		await dialog.getByRole("button", { name: "Fermer", exact: true }).click();
		await expect(dialog).toHaveCount(0);
		await page.locator("#chatInput").fill("en attente");
		await page.locator("#chatInput").press("Enter");
		const queued = page.locator("#queuedMessages [data-queued-seq]");
		await expect(queued.locator(".queued-label")).toHaveText("Suivant");
		await expect(queued.getByRole("button", { name: "Envoyer maintenant", exact: true })).toBeVisible();
		await expect(queued.getByRole("button", { name: "Retirer de la file", exact: true })).toBeVisible();

		expect(pageErrors).toEqual([]);
	});

	test("plural tool counts in Traditional Chinese", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await setLocale(page, "zh-TW");
		await chatEvent(page, {
			state: "tool_call_start",
			runId: "run-tw",
			toolCallId: "tw1",
			toolName: "Read",
			arguments: { file_path: "/tmp/a.txt" },
		});
		await chatEvent(page, {
			state: "tool_call_end",
			runId: "run-tw",
			toolCallId: "tw1",
			toolName: "Read",
			success: true,
			result: { content: "a\nb", total_lines: 2 },
		});
		const card = page.locator("#tool-run-tw-tw1");
		await expect(card.locator(".tool-card-verb")).toHaveText("已讀取");
		await expect(card.locator(".tool-card-badge")).toHaveText("2 行");
		expect(pageErrors).toEqual([]);
	});
});
