const { expect, test } = require("../base-test");
const {
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

/**
 * Answer the listed RPC methods locally and record their params in
 * `window.__rpcCalls`. A response can be an object, or an array consumed
 * one entry per call (the last entry repeats).
 */
async function mockRpcs(page, responses) {
	await page.evaluate((table) => {
		window.__rpcCalls = [];
		window.__rpcTable = table;
		const state = window.__antre_modules.state;
		const ws = state.ws;
		if (!window.__rpcOriginalSend) window.__rpcOriginalSend = ws.send.bind(ws);
		ws.send = (raw) => {
			const parsed = JSON.parse(raw);
			const entry = window.__rpcTable[parsed?.method];
			if (entry === undefined) return window.__rpcOriginalSend(raw);
			window.__rpcCalls.push({ method: parsed.method, params: parsed.params });
			const payload = Array.isArray(entry) ? (entry.length > 1 ? entry.shift() : entry[0]) : entry;
			const resolve = state.pending?.[parsed.id];
			if (typeof resolve === "function") {
				delete state.pending[parsed.id];
				resolve({ ok: true, payload });
			}
		};
	}, responses);
}

async function rpcCalls(page, method) {
	return page.evaluate((m) => (window.__rpcCalls || []).filter((c) => c.method === m).map((c) => c.params), method);
}

const HISTORY = [
	{ role: "user", content: "first question", historyIndex: 0, run_id: "r1" },
	{ role: "assistant", content: "first answer", historyIndex: 1, run_id: "r1", model: "m", provider: "p" },
	{ role: "user", content: "second question", historyIndex: 2, run_id: "r2" },
	{ role: "assistant", content: "second answer", historyIndex: 3, run_id: "r2", model: "m", provider: "p" },
];

const POINTS = [
	{ index: 0, text: "first question", hasAttachments: false, runIds: ["r1"] },
	{ index: 2, text: "second question", hasAttachments: false, runIds: ["r2"] },
];

function switchPayload(key, history) {
	return { entry: { key, label: key, messageCount: history.length }, history, replying: false };
}

async function loadHistory(page, history = HISTORY, key = "main") {
	await page.evaluate((k) => window.__antre_modules["stores/session-history-cache"]?.clearSessionHistory?.(k), key);
	await mockRpcs(page, { "sessions.switch": switchPayload(key, history) });
	await page.evaluate((k) => window.__antre_modules.sessions.switchSession(k), key);
	await expect(page.locator("#messages .msg.user")).toHaveCount(history.filter((m) => m.role === "user").length);
}

test.describe("Rewind, regenerate and edit", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
	});

	test.afterEach(async ({ page }) => {
		await page.evaluate(() => {
			const ws = window.__antre_modules?.state?.ws;
			if (ws && window.__rpcOriginalSend) ws.send = window.__rpcOriginalSend;
		});
	});

	test("rewinding the conversation truncates before the message and refills the composer", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await loadHistory(page);
		await mockRpcs(page, {
			"sessions.rewind_points": { points: POINTS },
			"sessions.truncate": { messageCount: 2, removed: 2 },
			"sessions.switch": switchPayload("main", HISTORY.slice(0, 2)),
		});
		const second = page.locator("#messages .msg.user").filter({ hasText: "second question" });
		await second.hover();
		await second.getByRole("button", { name: "Rewind", exact: true }).click();
		const dialog = page.getByRole("dialog", { name: "Rewind" });
		await expect(dialog).toBeVisible();
		await expect(dialog.locator(".rewind-quote")).toHaveText("second question");
		await dialog.getByRole("button", { name: "Restore conversation only", exact: true }).click();
		await expect(dialog).toHaveCount(0);
		expect(await rpcCalls(page, "sessions.truncate")).toEqual([{ key: "main", keep: 2 }]);
		expect(await rpcCalls(page, "checkpoints.restore")).toEqual([]);
		await expect(page.locator("#chatInput")).toHaveValue("second question");
		await expect(page.locator("#messages .msg.user")).toHaveCount(1);
		expect(pageErrors).toEqual([]);
	});

	test("restoring files undoes the runs from that message on", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await loadHistory(page);
		await mockRpcs(page, {
			"sessions.rewind_points": { points: POINTS },
			"checkpoints.restore": { restored: 3, paths: ["/repo/a.rs"], errors: [] },
		});
		const first = page.locator("#messages .msg.user").filter({ hasText: "first question" });
		await first.hover();
		await first.getByRole("button", { name: "Rewind", exact: true }).click();
		await page.getByRole("button", { name: "Restore files only", exact: true }).click();
		await expect(page.getByRole("dialog", { name: "Rewind" })).toHaveCount(0);
		expect(await rpcCalls(page, "checkpoints.restore")).toEqual([{ sessionKey: "main", runIds: ["r1", "r2"] }]);
		expect(await rpcCalls(page, "sessions.truncate")).toEqual([]);
		expect(pageErrors).toEqual([]);
	});

	test("editing a message forks before it, switches to the fork and sends the new text", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await loadHistory(page);
		await mockRpcs(page, {
			"sessions.rewind_points": { points: POINTS },
			"sessions.fork": { sessionKey: "session:e2e-fork", forkPoint: 2 },
			"sessions.switch": switchPayload("session:e2e-fork", HISTORY.slice(0, 2)),
			"chat.send": { runId: "run-fork" },
		});
		const second = page.locator("#messages .msg.user").filter({ hasText: "second question" });
		await second.hover();
		await second.getByRole("button", { name: "Edit message", exact: true }).click();
		const editor = second.getByRole("textbox", { name: "Edit message" });
		await editor.fill("second question, rephrased");
		await second.getByRole("button", { name: "Send as new branch", exact: true }).click();
		await expect.poll(() => rpcCalls(page, "chat.send")).toHaveLength(1);
		expect(await rpcCalls(page, "sessions.fork")).toEqual([{ key: "main", forkPoint: 2 }]);
		const [send] = await rpcCalls(page, "chat.send");
		expect(send.text).toBe("second question, rephrased");
		expect(send._session_key).toBe("session:e2e-fork");
		await expect.poll(() => new URL(page.url()).pathname).toBe("/chats/session/e2e-fork");
		await expect(page.locator("#messages .msg.user").last()).toContainText("second question, rephrased");
		expect(pageErrors).toEqual([]);
	});

	test("regenerate drops the last reply and asks again with the same message", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await loadHistory(page);
		await mockRpcs(page, {
			"sessions.rewind_points": { points: POINTS },
			"sessions.truncate": { messageCount: 2, removed: 2 },
			"sessions.switch": switchPayload("main", HISTORY.slice(0, 2)),
			"chat.send": { runId: "run-regen" },
		});
		const last = page.locator("#messages .msg.assistant").filter({ hasText: "second answer" });
		await last.hover();
		await last.locator('.msg-action-btn[title="Retry"]').click();
		await page.getByRole("button", { name: "Regenerate", exact: true }).click();
		await expect.poll(() => rpcCalls(page, "chat.send")).toHaveLength(1);
		expect(await rpcCalls(page, "sessions.truncate")).toEqual([{ key: "main", keep: 2 }]);
		const [send] = await rpcCalls(page, "chat.send");
		expect(send.text).toBe("second question");
		await expect(page.locator("#messages .msg.user")).toHaveCount(2);
		await expect(page.locator("#messages .msg.assistant").filter({ hasText: "second answer" })).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("Esc Esc opens the rewind picker", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await loadHistory(page);
		await mockRpcs(page, { "sessions.rewind_points": { points: POINTS } });
		// Both presses in one task: separate key presses can drift past the
		// double-press window on a loaded machine.
		await page.locator("#chatInput").evaluate((input) => {
			for (let i = 0; i < 2; i++) {
				input.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
			}
		});
		const dialog = page.getByRole("dialog", { name: "Rewind" });
		await expect(dialog).toBeVisible();
		await expect(dialog.locator(".rewind-turn")).toHaveCount(2);
		await expect(dialog.locator(".rewind-turn").first()).toContainText("second question");
		await dialog.locator(".rewind-turn").first().click();
		await expect(dialog.getByRole("button", { name: "Restore conversation and files", exact: true })).toBeVisible();
		await page.keyboard.press("Escape");
		await expect(dialog).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});
});
