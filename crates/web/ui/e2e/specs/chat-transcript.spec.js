const { expect, test } = require("../base-test");
const {
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

async function chatEvent(page, payload) {
	await expectRpcOk(page, "system-event", { event: "chat", payload: { sessionKey: "main", ...payload } });
}

/** Answer the next `method` RPC locally with `payload` (the server is not called). */
async function mockNextRpc(page, method, payload) {
	await page.evaluate(
		({ targetMethod, responsePayload }) => {
			const state = window.__moltis_modules.state;
			const ws = state.ws;
			const originalSend = ws.send.bind(ws);
			ws.send = (raw) => {
				const parsed = JSON.parse(raw);
				if (parsed?.method === targetMethod) {
					ws.send = originalSend;
					const resolve = state.pending?.[parsed.id];
					if (typeof resolve === "function") {
						delete state.pending[parsed.id];
						resolve({ ok: true, payload: responsePayload });
					}
					return;
				}
				originalSend(raw);
			};
		},
		{ targetMethod: method, responsePayload: payload },
	);
}

test.describe("Chat transcript components", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
		await expect(page.locator("#messages .msg.assistant")).toHaveCount(0);
	});

	test("streaming keeps earlier markdown blocks in place and settles into one reply", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const runId = "run-stream-blocks";
		await chatEvent(page, { state: "delta", runId, text: "# Plan\n\nFirst paragraph." });
		const reply = page.locator("#messages .msg.assistant");
		await expect(reply).toHaveCount(1);
		await expect(reply.locator("h1")).toHaveText("Plan");
		// Tag the heading's DOM node: a full re-render would replace it.
		await reply.locator("h1").evaluate((el) => {
			el.dataset.e2eMarker = "kept";
		});
		await chatEvent(page, { state: "delta", runId, text: "\n\nSecond paragraph" });
		await chatEvent(page, { state: "delta", runId, text: " grows." });
		await expect(reply).toContainText("Second paragraph grows.");
		await expect(reply.locator("h1")).toHaveAttribute("data-e2e-marker", "kept");

		await chatEvent(page, {
			state: "final",
			runId,
			text: "# Plan\n\nFirst paragraph.\n\nSecond paragraph grows.",
			messageIndex: 990001,
			model: "test-model",
			provider: "test-provider",
			replyMedium: "text",
		});
		await expect(reply).toHaveCount(1);
		await expect(reply).not.toHaveClass(/streaming/);
		await expect(reply.locator(".msg-model-footer")).toContainText("test-provider / test-model");
		await expect(reply.locator(".msg-action-bar")).toHaveCount(1);
		expect(pageErrors).toEqual([]);
	});

	test("a tool call between deltas splits the reply around its card", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const runId = "run-split";
		await chatEvent(page, { state: "delta", runId, text: "Let me look." });
		await chatEvent(page, {
			state: "tool_call_start",
			runId,
			toolCallId: "split-1",
			toolName: "exec",
			arguments: { command: "ls" },
		});
		await chatEvent(page, {
			state: "tool_call_end",
			runId,
			toolCallId: "split-1",
			toolName: "exec",
			success: true,
			result: { stdout: "a.txt", stderr: "", exit_code: 0 },
		});
		await chatEvent(page, { state: "delta", runId, text: "Found a.txt." });
		await chatEvent(page, {
			state: "final",
			runId,
			text: "Found a.txt.",
			messageIndex: 990002,
			model: "test-model",
			provider: "test-provider",
		});
		const order = await page
			.locator("#messages .msg")
			.evaluateAll((els) =>
				els.map((el) => (el.classList.contains("exec-card") ? "tool" : el.textContent.trim().slice(0, 12))),
			);
		expect(order).toEqual(["Let me look.", "tool", "Found a.txt."]);
		expect(pageErrors).toEqual([]);
	});

	test("switching back to a running session resumes its streamed text", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await mockNextRpc(page, "sessions.switch", {
			entry: { key: "main", label: "Main", messageCount: 0 },
			history: [],
			replying: true,
			streamText: "Partial answer so far",
		});
		await page.evaluate(() => window.__moltis_modules.sessions.switchSession("main"));
		const reply = page.locator("#messages .msg.assistant.streaming");
		await expect(reply).toContainText("Partial answer so far");
		await chatEvent(page, { state: "delta", runId: "run-resume", text: " and more." });
		await expect(reply).toContainText("Partial answer so far and more.");
		await expect(page.locator("#messages .msg.assistant:not(.thinking)")).toHaveCount(1);
		await chatEvent(page, { state: "aborted", runId: "run-resume" });
		expect(pageErrors).toEqual([]);
	});

	test("the session header is mounted once", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await expect(page.locator(".chat-toolbar .chat-session-name")).toHaveCount(1);
		await expect(page.getByRole("button", { name: "Session actions", exact: true })).toHaveCount(1);
		expect(pageErrors).toEqual([]);
	});

	test("status notices are removed by id, not by position", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await expectRpcOk(page, "system-event", { event: "sandbox.image.provision", payload: { phase: "start" } });
		await expect(page.getByText("Provisioning sandbox packages…", { exact: true })).toBeVisible();
		// Something else lands after the status notice before its phase ends.
		await chatEvent(page, {
			state: "final",
			runId: "run-notice",
			text: "unrelated reply",
			messageIndex: 990003,
			model: "test-model",
			provider: "test-provider",
		});
		await expectRpcOk(page, "system-event", { event: "sandbox.image.provision", payload: { phase: "done" } });
		await expect(page.getByText("Provisioning sandbox packages…", { exact: true })).toHaveCount(0);
		await expect(page.getByText("Sandbox packages provisioned", { exact: true })).toBeVisible();
		await expect(page.locator("#messages .msg.assistant").filter({ hasText: "unrelated reply" })).toHaveCount(1);
		expect(pageErrors).toEqual([]);
	});
});
