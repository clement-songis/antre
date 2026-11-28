const { expect, test } = require("../base-test");
const {
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

/** Answer the listed RPCs locally and record their params in `window.__rpcCalls`. */
async function mockRpcs(page, responses) {
	await page.evaluate((table) => {
		window.__rpcCalls = [];
		window.__rpcTable = table;
		const state = window.__antre_modules.state;
		const ws = state.ws;
		if (!window.__rpcOriginalSend) window.__rpcOriginalSend = ws.send.bind(ws);
		ws.send = (raw) => {
			const parsed = JSON.parse(raw);
			const payload = window.__rpcTable[parsed?.method];
			if (payload === undefined) return window.__rpcOriginalSend(raw);
			window.__rpcCalls.push({ method: parsed.method, params: parsed.params });
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

async function queueMessages(page, texts) {
	for (const text of texts) {
		await page.locator("#chatInput").fill(text);
		await page.locator("#chatInput").press("Enter");
		await expect(page.locator(`#queuedMessages [data-queued-seq]`).filter({ hasText: text })).toHaveCount(1);
	}
}

async function seqOf(page, text) {
	return Number(
		await page.locator("#queuedMessages [data-queued-seq]").filter({ hasText: text }).getAttribute("data-queued-seq"),
	);
}

test.describe("Queued messages", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
		await mockRpcs(page, {
			"chat.send": { queued: true },
			"chat.cancel_queued": { cleared: 1 },
			"chat.steer": { steered: true },
		});
	});

	test.afterEach(async ({ page }) => {
		await page.evaluate(() => {
			const ws = window.__antre_modules?.state?.ws;
			if (ws && window.__rpcOriginalSend) ws.send = window.__rpcOriginalSend;
		});
	});

	test("queued messages wait in the tray, not in the transcript", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await queueMessages(page, ["first queued", "second queued"]);
		const tray = page.locator("#queuedMessages");
		await expect(tray).not.toHaveClass(/\bhidden\b/);
		await expect(tray.locator(".queued-label").first()).toHaveText("Next");
		await expect(page.locator("#messages .msg.user")).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("remove and edit act on one queued message", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await queueMessages(page, ["drop me", "fix me", "keep me"]);
		const dropSeq = await seqOf(page, "drop me");
		await page
			.locator("#queuedMessages [data-queued-seq]")
			.filter({ hasText: "drop me" })
			.getByRole("button", { name: "Remove from the queue", exact: true })
			.click();
		await expect(page.locator("#queuedMessages").getByText("drop me", { exact: true })).toHaveCount(0);
		const fixSeq = await seqOf(page, "fix me");
		await page
			.locator("#queuedMessages [data-queued-seq]")
			.filter({ hasText: "fix me" })
			.getByRole("button", { name: "Edit", exact: true })
			.click();
		await expect(page.locator("#chatInput")).toHaveValue("fix me");
		await expect(page.locator("#queuedMessages [data-queued-seq]")).toHaveCount(1);
		expect(await rpcCalls(page, "chat.cancel_queued")).toEqual([
			{ sessionKey: "main", seq: dropSeq },
			{ sessionKey: "main", seq: fixSeq },
		]);
		expect(pageErrors).toEqual([]);
	});

	test("send now steers the message to the front and it is shown when replayed", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await queueMessages(page, ["later", "urgent"]);
		const urgentSeq = await seqOf(page, "urgent");
		await page
			.locator("#queuedMessages [data-queued-seq]")
			.filter({ hasText: "urgent" })
			.getByRole("button", { name: "Send now", exact: true })
			.click();
		expect(await rpcCalls(page, "chat.steer")).toEqual([{ sessionKey: "main", seq: urgentSeq }]);
		await expect(page.locator("#queuedMessages [data-queued-seq]").first()).toContainText("urgent");
		// The steer aborts the running reply; the server then replays "urgent".
		await expectRpcOk(page, "system-event", {
			event: "chat",
			payload: { sessionKey: "main", state: "aborted", runId: "run-steer" },
		});
		await expect(page.locator("#messages .msg.user")).toHaveText(["urgent"]);
		await expect(page.locator("#queuedMessages [data-queued-seq]")).toHaveCount(1);
		expect(pageErrors).toEqual([]);
	});

	test("a removal announced by the server updates the tray", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await queueMessages(page, ["from another tab"]);
		const seq = await seqOf(page, "from another tab");
		await expectRpcOk(page, "system-event", {
			event: "chat",
			payload: { sessionKey: "main", state: "queue_item_removed", seq },
		});
		await expect(page.locator("#queuedMessages")).toHaveClass(/\bhidden\b/);
		expect(pageErrors).toEqual([]);
	});
});
