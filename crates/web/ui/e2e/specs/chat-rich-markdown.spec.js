const { expect, test } = require("../base-test");
const {
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

async function finalReply(page, text, messageIndex) {
	await expectRpcOk(page, "system-event", {
		event: "chat",
		payload: {
			sessionKey: "main",
			state: "final",
			runId: `run-${messageIndex}`,
			text,
			messageIndex,
			replyMedium: "text",
		},
	});
}

test.describe("Rich markdown", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
	});

	test("mermaid fences render as diagrams", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await finalReply(page, "Flow:\n\n```mermaid\ngraph TD\n  A[Start] --> B[Done]\n```", 992001);
		const diagram = page.locator("#messages .msg.assistant .mermaid-diagram svg");
		await expect(diagram).toBeVisible({ timeout: 15_000 });
		await expect(page.locator("#messages .msg.assistant pre.mermaid-source")).toBeHidden();
		expect(pageErrors).toEqual([]);
	});

	test("TeX renders with KaTeX and prices stay text", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await finalReply(page, "Energy:\n\n$$E = mc^2$$\n\nInline $a_1^2$ here, and it costs $5 or $10.", 992002);
		const reply = page.locator("#messages .msg.assistant");
		await expect(reply.locator(".math-display .katex")).toHaveCount(1, { timeout: 15_000 });
		await expect(reply.locator(".math-inline .katex")).toHaveCount(1);
		await expect(reply).toContainText("it costs $5 or $10.");
		expect(pageErrors).toEqual([]);
	});
});
