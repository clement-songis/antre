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
			const state = window.__antre_modules.state;
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

async function toolCall(page, { runId, id, name, args, result, success = true, error }) {
	await chatEvent(page, { state: "tool_call_start", runId, toolCallId: id, toolName: name, arguments: args });
	if (result === undefined && error === undefined) return;
	await chatEvent(page, {
		state: "tool_call_end",
		runId,
		toolCallId: id,
		toolName: name,
		arguments: args,
		success,
		result,
		error: error ? { detail: error } : undefined,
	});
}

test.describe("Tool call cards", () => {
	test.beforeEach(async ({ page }) => {
		await navigateAndWait(page, "/chats/main");
		await waitForWsConnected(page);
		await waitForChatSessionReady(page);
		await expectRpcOk(page, "chat.clear", {});
	});

	test("an edit card shows the file, a +/- badge and an inline diff when expanded", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await toolCall(page, {
			runId: "run-edit",
			id: "edit-1",
			name: "Edit",
			args: {
				file_path: "/repo/src/lib.rs",
				old_string: "fn a() {}\nfn b() {}\n",
				new_string: "fn a() {}\nfn c() {}\nfn d() {}\n",
			},
			result: { file_path: "/repo/src/lib.rs", replacements: 1 },
		});
		const card = page.locator("#tool-run-edit-edit-1");
		await expect(card.locator(".tool-card-verb")).toHaveText("Edited");
		await expect(card.locator(".tool-card-target")).toContainText("src/lib.rs");
		await expect(card.locator(".tool-diff-badge")).toHaveText("+2 −1");
		const body = card.locator(".tool-card-body");
		await expect(body).toBeHidden();
		await card.locator(".tool-card-header").click();
		await expect(body).toBeVisible();
		await expect(body.locator(".tool-diff-del")).toHaveText("- fn b() {}");
		await expect(body.locator(".tool-diff-add")).toHaveCount(2);
		expect(pageErrors).toEqual([]);
	});

	test("consecutive calls fold into a group that collapses when done", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const runId = "run-group";
		await toolCall(page, {
			runId,
			id: "r1",
			name: "Read",
			args: { file_path: "/repo/a.rs" },
			result: { content: "1→a", total_lines: 1, start_line: 1 },
		});
		await toolCall(page, { runId, id: "g1", name: "Grep", args: { pattern: "TODO", path: "/repo" } });
		const group = page.locator(".tool-group");
		await expect(group).toHaveAttribute("data-tool-count", "2");
		await expect(group.locator(".tool-group-summary")).toHaveText("1 read, 1 search");
		// Still running: the group stays open.
		await expect(page.locator("#tool-run-group-g1")).toBeVisible();
		await chatEvent(page, {
			state: "tool_call_end",
			runId,
			toolCallId: "g1",
			toolName: "Grep",
			success: true,
			result: { mode: "files_with_matches", files: ["/repo/a.rs", "/repo/b.rs"], truncated: false },
		});
		await chatEvent(page, {
			state: "final",
			runId,
			text: "Two files have TODOs.",
			messageIndex: 991101,
			model: "test-model",
			provider: "test-provider",
		});
		await expect(page.locator("#tool-run-group-g1")).toBeHidden();
		await group.locator(".tool-group-header").click();
		await expect(page.locator("#tool-run-group-g1 .tool-card-badge")).toHaveText("2 files");
		expect(pageErrors).toEqual([]);
	});

	test("a running card shows elapsed time and a failed card opens on its error", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await toolCall(page, { runId: "run-time", id: "x1", name: "exec", args: { command: "sleep 2" } });
		const card = page.locator("#tool-run-time-x1");
		await expect(card.locator(".exec-status")).toBeVisible();
		await expect(card.locator(".tool-card-elapsed")).toHaveText(/\d/);
		await chatEvent(page, {
			state: "tool_call_end",
			runId: "run-time",
			toolCallId: "x1",
			toolName: "exec",
			success: false,
			error: { detail: "permission denied" },
		});
		await expect(card).toHaveClass(/exec-err/);
		await expect(card.locator(".exec-error-detail")).toBeVisible();
		await expect(card.locator(".exec-error-detail")).toHaveText("permission denied");
		expect(pageErrors).toEqual([]);
	});

	test("task_list calls render as a live checklist pinned under the transcript", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const runId = "run-tasks";
		await toolCall(page, {
			runId,
			id: "t1",
			name: "task_list",
			args: { action: "create", subject: "Write tests" },
			result: { ok: true, task: { id: "1", subject: "Write tests", status: "pending", updated_at: 1 } },
		});
		await toolCall(page, {
			runId,
			id: "t2",
			name: "task_list",
			args: { action: "create", subject: "Ship it" },
			result: { ok: true, task: { id: "2", subject: "Ship it", status: "pending", updated_at: 2 } },
		});
		await toolCall(page, {
			runId,
			id: "t3",
			name: "task_list",
			args: { action: "update", id: "1", status: "completed" },
			result: { ok: true, task: { id: "1", subject: "Write tests", status: "completed", updated_at: 3 } },
		});
		const pinned = page.locator(".pinned-task-checklist");
		await expect(pinned).toBeVisible();
		await expect(pinned.locator(".tool-section-label")).toHaveText("Tasks · 1/2 done");
		await expect(pinned.locator('[data-task-status="completed"]')).toHaveText(/Write tests/);
		await expect(pinned.locator('[data-task-status="pending"]')).toHaveText(/Ship it/);
		await toolCall(page, {
			runId,
			id: "t4",
			name: "task_list",
			args: { action: "update", id: "2", status: "completed" },
			result: { ok: true, task: { id: "2", subject: "Ship it", status: "completed", updated_at: 4 } },
		});
		await expect(pinned).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("MCP and unknown tools show their arguments", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await toolCall(page, {
			runId: "run-mcp",
			id: "m1",
			name: "mcp__github__create_issue",
			args: { title: "Bug" },
			result: { number: 7 },
		});
		const card = page.locator("#tool-run-mcp-m1");
		await expect(card.locator(".tool-card-verb")).toHaveText("create_issue");
		await expect(card.locator(".tool-card-target")).toHaveText("via github");
		await card.locator(".tool-card-header").click();
		await expect(card.locator(".tool-json").first()).toContainText('"title": "Bug"');
		expect(pageErrors).toEqual([]);
	});

	test("reloaded history renders tool calls with the same renderers", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.evaluate(() => window.__antre_modules["stores/session-history-cache"]?.clearSessionHistory?.("main"));
		await mockNextRpc(page, "sessions.switch", {
			entry: { key: "main", label: "Main", messageCount: 3 },
			history: [
				{ role: "user", content: "rename b to c", historyIndex: 0 },
				{
					role: "tool_result",
					tool_call_id: "hist-edit",
					tool_name: "Edit",
					arguments: { file_path: "/repo/src/lib.rs", old_string: "fn b() {}\n", new_string: "fn c() {}\n" },
					success: true,
					result: { file_path: "/repo/src/lib.rs", replacements: 1 },
					run_id: "run-hist",
					historyIndex: 1,
				},
				{ role: "assistant", content: "Renamed.", run_id: "run-hist", historyIndex: 2 },
			],
			replying: false,
		});
		await page.evaluate(() => window.__antre_modules.sessions.switchSession("main"));
		const card = page.locator('#messages .tool-card[data-tool-name="Edit"]');
		await expect(card.locator(".tool-card-verb")).toHaveText("Edited");
		await expect(card.locator(".tool-diff-badge")).toHaveText("+1 \u22121");
		await expect(page.locator("#messages .msg.assistant").filter({ hasText: "Renamed." })).toHaveCount(1);
		expect(pageErrors).toEqual([]);
	});
});
