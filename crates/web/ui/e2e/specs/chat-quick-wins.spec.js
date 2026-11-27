const { expect, test } = require("../base-test");
const {
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

async function broadcast(page, event, payload) {
	await expectRpcOk(page, "system-event", { event, payload });
}

async function chatEvent(page, payload) {
	await broadcast(page, "chat", { sessionKey: "main", ...payload });
}

test.describe("Chat quick wins", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
	});

	test("approval dock counts down from the server expiry and Enter allows once", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await broadcast(page, "exec.approval.requested", {
			requestId: "req-enter",
			command: "rm -rf build",
			sessionKey: "main",
			expiresAtMs: Date.now() + 45_000,
		});

		const card = page.locator('.approval-dock [data-request-id="req-enter"]');
		await expect(card).toBeVisible();
		await expect(card.locator(".approval-cmd")).toHaveText("rm -rf build");
		await expect(card.getByRole("button", { name: "Always allow", exact: true })).toBeVisible();
		await expect(card.locator(".approval-countdown")).toHaveText(/^(4[0-5])s$/);

		await page.locator("#chatInput").focus();
		await page.keyboard.press("Enter");
		await expect(card).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("resolved event removes the card and other sessions show a badge", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await broadcast(page, "exec.approval.requested", {
			requestId: "req-resolved",
			command: "make deploy",
			sessionKey: "main",
		});
		const card = page.locator('[data-request-id="req-resolved"]');
		await expect(card).toBeVisible();
		await broadcast(page, "exec.approval.resolved", {
			requestId: "req-resolved",
			decision: "timeout",
			sessionKey: "main",
		});
		await expect(card).toHaveCount(0);

		await broadcast(page, "exec.approval.requested", {
			requestId: "req-other",
			command: "ls",
			sessionKey: "session:elsewhere",
		});
		await expect(page.locator(".approvals-badge")).toBeVisible();
		await expect(page.locator(".approvals-badge")).toHaveText("1");
		await expect(page.locator('[data-request-id="req-other"]')).toHaveCount(0);
		await broadcast(page, "exec.approval.resolved", { requestId: "req-other" });
		await expect(page.locator(".approvals-badge")).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("Escape denies a pending approval before stopping the run", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await chatEvent(page, { state: "thinking", runId: "run-esc-approval" });
		await broadcast(page, "exec.approval.requested", {
			requestId: "req-esc",
			command: "curl example.com",
			sessionKey: "main",
		});
		await expect(page.locator('[data-request-id="req-esc"]')).toBeVisible();
		await page.locator("#chatInput").focus();
		await page.keyboard.press("Escape");
		await expect(page.locator('[data-request-id="req-esc"]')).toHaveCount(0);
		await expect(page.locator("#sendBtn")).toHaveAttribute("data-mode", "stop");
		await expect(page.locator("#sendBtn")).not.toHaveClass(/is-stopping/);
		await chatEvent(page, { state: "aborted", runId: "run-esc-approval" });
		expect(pageErrors).toEqual([]);
	});

	test("Escape stops a running generation", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await chatEvent(page, { state: "thinking", runId: "run-esc-stop" });
		const sendBtn = page.locator("#sendBtn");
		await expect(sendBtn).toHaveAttribute("data-mode", "stop");
		await page.locator("#chatInput").focus();
		await page.keyboard.press("Escape");
		await expect(sendBtn).toHaveClass(/is-stopping/);
		await chatEvent(page, { state: "aborted", runId: "run-esc-stop" });
		await expect(sendBtn).toHaveAttribute("data-mode", "send");
		expect(pageErrors).toEqual([]);
	});

	test("tools still running when the run is aborted are shown as cancelled", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await chatEvent(page, {
			state: "tool_call_start",
			runId: "run-cancel",
			toolCallId: "tc-1",
			toolName: "Read",
			arguments: { file_path: "/repo/src/main.rs" },
		});
		const card = page.locator("#tool-run-cancel-tc-1");
		await expect(card).toBeVisible();
		await expect(card.locator(".tool-card-target")).toHaveText("/repo/src/main.rs");
		await chatEvent(page, { state: "aborted", runId: "run-cancel" });
		await expect(card).toHaveClass(/exec-cancelled/);
		await expect(card).toContainText("Cancelled");
		expect(pageErrors).toEqual([]);
	});

	test("slash menu inserts commands that take arguments instead of sending them", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const input = page.locator("#chatInput");
		await input.fill("");
		await input.pressSequentially("/mo");
		await expect(page.locator(".slash-menu-item.active .slash-name")).toHaveText("/mode");
		await page.keyboard.press("Enter");
		await expect(input).toHaveValue("/mode ");
		await expect(page.locator(".slash-menu")).toHaveCount(0);
		await expect(page.locator("#messages .msg.user")).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("/rollback lists checkpoints in the web UI", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const input = page.locator("#chatInput");
		await input.fill("/rollback");
		await page.keyboard.press("Escape");
		await page.keyboard.press("Enter");
		await expect(page.getByText("No file checkpoints yet", { exact: false })).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("code blocks get a copy button", async ({ page, context }) => {
		const pageErrors = watchPageErrors(page);
		await context.grantPermissions(["clipboard-read", "clipboard-write"]).catch(() => undefined);
		await chatEvent(page, {
			state: "final",
			runId: "run-code-copy",
			text: "Here:\n\n```sh\necho copied\n```",
			messageIndex: 9001,
		});
		const button = page.locator("#messages pre.code-block .code-copy-btn").last();
		await expect(button).toHaveCount(1);
		await button.click();
		await expect(button).toHaveText("Copied");
		expect(pageErrors).toEqual([]);
	});

	test("live reasoning shows its tail", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await chatEvent(page, { state: "thinking", runId: "run-tail" });
		const lines = Array.from({ length: 12 }, (_, i) => `step ${i + 1}`).join("\n");
		await chatEvent(page, { state: "thinking_text", runId: "run-tail", text: lines });
		const text = page.locator("#thinkingIndicator .thinking-text");
		await expect(text).toHaveClass(/is-overflowing/);
		const atBottom = await text.evaluate((el) => el.scrollTop + el.clientHeight >= el.scrollHeight - 1);
		expect(atBottom).toBe(true);
		await chatEvent(page, { state: "aborted", runId: "run-tail" });
		expect(pageErrors).toEqual([]);
	});
});

test.describe("Chat quick wins on mobile", () => {
	test.use({ viewport: { width: 375, height: 812 } });

	test("send button stays visible on phones", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await expect(page.locator("#sendBtn")).toBeVisible();
		expect(pageErrors).toEqual([]);
	});
});
