const fs = require("node:fs");
const path = require("node:path");
const { expect, test } = require("../base-test");
const {
	expectRpcOk,
	navigateAndWait,
	waitForChatSessionReady,
	waitForWsConnected,
	watchPageErrors,
} = require("../helpers");

const DESIGN_HTML =
	'<!doctype html><html><head><meta charset="utf-8"><title>Landing</title></head>' +
	'<body><h1 data-mid="hero">Hello canvas</h1><p data-mid="lede">Welcome to the preview</p></body></html>';

async function createDesign(page, title) {
	const response = await expectRpcOk(page, "canvas.create", { title, sessionKey: "main", html: DESIGN_HTML });
	return response.payload.design;
}

function pane(page) {
	return page.getByRole("complementary", { name: "Design preview", exact: true });
}

function designFrame(page) {
	return page.frameLocator('[data-testid="canvas-frame"]');
}

/** Capture outgoing `chat.send` texts and answer them locally. */
async function captureChatSends(page) {
	await page.evaluate(() => {
		const state = window.__moltis_modules.state;
		const ws = state.ws;
		const originalSend = ws.send.bind(ws);
		window.__canvasSent = [];
		ws.send = (raw) => {
			const parsed = JSON.parse(raw);
			if (parsed?.method === "chat.send") {
				window.__canvasSent.push(parsed.params.text);
				const resolve = state.pending?.[parsed.id];
				if (typeof resolve === "function") {
					delete state.pending[parsed.id];
					resolve({ ok: true, payload: { runId: `canvas-${window.__canvasSent.length}` } });
				}
				return;
			}
			originalSend(raw);
		};
	});
}

async function sentTexts(page) {
	return page.evaluate(() => window.__canvasSent || []);
}

/** Answer `method` RPCs locally with `payload` for the rest of the test. */
async function mockRpc(page, method, payload) {
	await page.evaluate(
		({ targetMethod, responsePayload }) => {
			const state = window.__moltis_modules.state;
			const ws = state.ws;
			const originalSend = ws.send.bind(ws);
			ws.send = (raw) => {
				const parsed = JSON.parse(raw);
				if (parsed?.method === targetMethod) {
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

test.describe("Design canvas pane", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
	});

	test("opens when a design is created and renders it on the sandboxed render origin", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await createDesign(page, "E2E landing");

		await expect(pane(page)).toBeVisible();
		await expect(page.getByTestId("canvas-title")).toHaveText("E2E landing");
		await expect(designFrame(page).getByText("Hello canvas", { exact: true })).toBeVisible();

		const frame = page.getByTestId("canvas-frame");
		await expect(frame).toHaveAttribute("sandbox", "allow-scripts");
		const src = await frame.getAttribute("src");
		const pageOrigin = new URL(page.url()).origin;
		expect(new URL(src).origin).not.toBe(pageOrigin);
		expect(src).toContain(`#host=${encodeURIComponent(pageOrigin)}`);

		await page.getByRole("button", { name: "Close preview", exact: true }).click();
		await expect(pane(page)).toBeHidden();
		await page.getByTestId("canvas-toggle").click();
		await expect(pane(page)).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("reloads on new versions, shows past versions and reverts", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const design = await createDesign(page, "Versions");
		await expect(designFrame(page).getByText("Hello canvas", { exact: true })).toBeVisible();

		const details = await expectRpcOk(page, "canvas.get", { designId: design.id });
		fs.writeFileSync(path.join(details.payload.dir, "index.html"), DESIGN_HTML.replace("Hello canvas", "Version two"));
		// Publishing records the live files as a new version first.
		await page.getByRole("button", { name: "Publish", exact: true }).click();
		await page.getByRole("menuitem", { name: "Private link (with access key)", exact: true }).click();
		await expect(page.getByTestId("canvas-published")).toContainText("Version 2 published as a private link");
		await expect(page.getByRole("textbox", { name: "Published link", exact: true })).toHaveValue(/\/p\/[^/]+\?k=/);

		await expect(designFrame(page).getByText("Version two", { exact: true })).toBeVisible();
		const picker = page.getByRole("combobox", { name: "Version", exact: true });
		await expect(picker.locator("option:checked")).toHaveText("Latest (v2)");

		await picker.selectOption("1");
		await expect(designFrame(page).getByText("Hello canvas", { exact: true })).toBeVisible();
		await page.getByRole("button", { name: "Revert to v1", exact: true }).click();
		await expect(picker.locator("option:checked")).toHaveText("Latest (v3)");
		await expect(designFrame(page).getByText("Hello canvas", { exact: true })).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("picking an element attaches it to the next message", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await createDesign(page, "Pick");
		await expect(designFrame(page).getByText("Hello canvas", { exact: true })).toBeVisible();
		await captureChatSends(page);

		await page.getByRole("button", { name: "Select", exact: true }).click();
		await designFrame(page).getByText("Hello canvas", { exact: true }).click();
		const chips = page.getByRole("list", { name: "Elements attached to your message", exact: true });
		await expect(chips.getByText("hero", { exact: true })).toBeVisible();

		await page.locator("#chatInput").fill("Make this bigger");
		await page.locator("#chatInput").press("Enter");
		await expect.poll(() => sentTexts(page)).toHaveLength(1);
		const [text] = await sentTexts(page);
		expect(text).toContain("Make this bigger");
		expect(text).toContain('`[data-mid="hero"]` <h1> "Hello canvas"');
		expect(text).toContain('design "Pick"');
		await expect(chips).toBeHidden();
		expect(pageErrors).toEqual([]);
	});

	test("comment pins are stored per version and sent to the agent together", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await createDesign(page, "Comments");
		await expect(designFrame(page).getByText("Hello canvas", { exact: true })).toBeVisible();
		await captureChatSends(page);

		await page.getByRole("button", { name: "Comment", exact: true }).click();
		await designFrame(page).getByText("Welcome to the preview", { exact: true }).click();
		await page.getByRole("textbox", { name: "Comment", exact: true }).fill("Warmer tone here");
		await page.getByRole("button", { name: "Add comment", exact: true }).click();

		const comments = page.getByTestId("canvas-comments");
		await expect(comments.getByText("Warmer tone here", { exact: true })).toBeVisible();
		await expect(comments.getByText("1 open comment", { exact: true })).toBeVisible();
		await expect(designFrame(page).getByRole("button", { name: "Comment 1", exact: true })).toBeVisible();

		await comments.getByRole("button", { name: "Send to agent", exact: true }).click();
		await expect.poll(() => sentTexts(page)).toHaveLength(1);
		const [text] = await sentTexts(page);
		expect(text).toContain("Please address these comments");
		expect(text).toContain('1. On `[data-mid="lede"]` ("Welcome to the preview"): Warmer tone here');
		await expect(comments.getByText("Warmer tone here", { exact: true })).toBeHidden();
		expect(pageErrors).toEqual([]);
	});

	test("viewport toggle, fullscreen and exports", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const design = await createDesign(page, "Layout");
		await expect(designFrame(page).getByText("Hello canvas", { exact: true })).toBeVisible();

		await page.getByRole("button", { name: "Phone (390 px)", exact: true }).click();
		await expect
			.poll(async () => Math.round((await page.getByTestId("canvas-frame").boundingBox())?.width ?? 0))
			.toBe(390);

		await page.getByRole("button", { name: "Full screen", exact: true }).click();
		const viewportSize = page.viewportSize();
		await expect.poll(async () => Math.round((await pane(page).boundingBox())?.width ?? 0)).toBe(viewportSize.width);
		await page.keyboard.press("Escape");
		await expect(page.getByRole("button", { name: "Full screen", exact: true })).toBeVisible();

		await page.getByRole("button", { name: "Export", exact: true }).click();
		const downloadPromise = page.waitForEvent("download");
		await page.getByRole("menuitem", { name: "HTML (single file)", exact: true }).click();
		const download = await downloadPromise;
		expect(download.suggestedFilename()).toMatch(/\.html$/);
		expect(download.url()).toContain(`/api/canvas/designs/${design.id}/export/html`);
		expect(pageErrors).toEqual([]);
	});
});

test.describe("Inline sandboxed frames", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
	});

	test("HTML code blocks in replies preview in a sandboxed frame", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await expectRpcOk(page, "system-event", {
			event: "chat",
			payload: {
				sessionKey: "main",
				state: "final",
				runId: "run-artifact",
				text: "Here it is:\n\n```html\n<p>Snippet from the agent</p>\n```",
				messageIndex: 993001,
				replyMedium: "text",
			},
		});
		const preview = page.getByRole("button", { name: "Preview", exact: true });
		await preview.click();
		const frame = page.getByTestId("artifact-frame");
		await expect(frame).toHaveAttribute("sandbox", "allow-scripts");
		await expect(
			page.frameLocator('[data-testid="artifact-frame"]').getByText("Snippet from the agent", { exact: true }),
		).toBeVisible();
		await page.getByRole("button", { name: "Hide preview", exact: true }).click();
		await expect(frame).toBeHidden();
		expect(pageErrors).toEqual([]);
	});

	test("MCP App views render inline in their tool card", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const appHtml = `<!doctype html><html><body><div id="out">waiting</div><script>
			const send = (message) => window.parent.postMessage(message, "*");
			window.addEventListener("message", (event) => {
				const data = event.data;
				if (data && data.id === 1 && data.result) send({ jsonrpc: "2.0", method: "ui/notifications/initialized" });
				if (data && data.method === "ui/notifications/tool-result") {
					document.getElementById("out").textContent = "Result: " + data.params.structuredContent.total;
					send({ jsonrpc: "2.0", method: "ui/notifications/size-changed", params: { height: 120 } });
				}
			});
			send({ jsonrpc: "2.0", id: 1, method: "ui/initialize", params: { protocolVersion: "2026-01-26" } });
		</script></body></html>`;
		await mockRpc(page, "mcp.tools", [
			{ name: "chart", inputSchema: {}, _meta: { ui: { resourceUri: "ui://demo/chart" } } },
		]);
		await mockRpc(page, "mcp.apps.resource", {
			server: "demo",
			uri: "ui://demo/chart",
			mimeType: "text/html;profile=mcp-app",
			html: appHtml,
		});
		const base = { sessionKey: "main", runId: "run-app", toolCallId: "app-1", toolName: "mcp__demo__chart" };
		await expectRpcOk(page, "system-event", {
			event: "chat",
			payload: { ...base, state: "tool_call_start", arguments: { query: "sales" } },
		});
		await expectRpcOk(page, "system-event", {
			event: "chat",
			payload: { ...base, state: "tool_call_end", arguments: { query: "sales" }, success: true, result: { total: 42 } },
		});
		const app = page.frameLocator('[data-testid="mcp-app-frame"]');
		await expect(app.getByText("Result: 42", { exact: true })).toBeVisible();
		await expect(page.getByTestId("mcp-app-frame")).toHaveAttribute("sandbox", "allow-scripts");
		await expect.poll(async () => (await page.getByTestId("mcp-app-frame").boundingBox())?.height ?? 0).toBe(120);
		expect(pageErrors).toEqual([]);
	});
});
