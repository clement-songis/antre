// ── Context menus for transcript items ──────────────────────

import type { MenuSpec } from "../../../components/ContextMenu";
import { sendRpc } from "../../../helpers";
import { fetchSessions, switchSession } from "../../../sessions";
import { sessionStore } from "../../../stores/session-store";
import { copyToClipboard, showToast } from "../../../ui";
import { regenerateLastTurn } from "../rewind";
import type { AssistantItem, ToolItem, UserItem } from "./model";
import { transcriptItems } from "./store";
import { toolView } from "./tools/registry";
import { editingUserItemId, openRewindFor, resolvePoint } from "./UserActions";

function replying(sessionKey: string): boolean {
	return sessionStore.getByKey(sessionKey)?.replying.value === true;
}

function copy(text: string): void {
	void copyToClipboard(text, "Copied", "Could not copy \u2014 please copy manually.");
}

/** Fork the session so it keeps the first `forkPoint` messages, and open the fork. */
async function forkAt(sessionKey: string, forkPoint: number): Promise<void> {
	const res = await sendRpc<{ sessionKey?: string }>("sessions.fork", { key: sessionKey, forkPoint });
	const forkKey = res?.payload?.sessionKey;
	if (!(res?.ok && forkKey)) {
		showToast(res?.error?.message || "Fork failed", "error");
		return;
	}
	fetchSessions();
	switchSession(forkKey);
}

export function userMessageMenu(item: UserItem, sessionKey: string): MenuSpec {
	const busy = replying(sessionKey);
	const hasAttachments = item.images.length > 0 || item.documents.length > 0;
	return {
		label: "Message",
		items: [
			{ id: "copy", label: "Copy", icon: "icon-copy", onSelect: () => copy(item.text) },
			{
				id: "edit",
				label: "Edit and resend",
				icon: "icon-code",
				disabled: busy || hasAttachments,
				onSelect: () => {
					editingUserItemId.value = item.id;
				},
			},
			{
				id: "fork",
				label: "Fork from here",
				icon: "icon-git-fork",
				onSelect: () => {
					resolvePoint(item, sessionKey)
						.then(({ point }) => forkAt(sessionKey, point.index + 1))
						.catch((err: unknown) => showToast(err instanceof Error ? err.message : "Fork failed", "error"));
				},
			},
			{
				id: "rewind",
				label: "Rewind to before this",
				icon: "icon-retry",
				disabled: busy,
				separatorBefore: true,
				onSelect: () => void openRewindFor(item, sessionKey),
			},
		],
	};
}

function isLastReply(item: AssistantItem): boolean {
	const replies = transcriptItems.value.filter((entry) => entry.kind === "assistant");
	return replies[replies.length - 1]?.id === item.id;
}

export function assistantMessageMenu(item: AssistantItem, sessionKey: string): MenuSpec {
	const busy = replying(sessionKey);
	return {
		label: "Reply",
		items: [
			{ id: "copy", label: "Copy as Markdown", icon: "icon-copy", onSelect: () => copy(item.text) },
			{
				id: "regenerate",
				label: "Regenerate",
				icon: "icon-retry",
				disabled: busy || !item.final || !isLastReply(item),
				onSelect: () => {
					regenerateLastTurn(sessionKey).catch((err: unknown) =>
						showToast(err instanceof Error ? err.message : "Regenerate failed", "error"),
					);
				},
			},
			{
				id: "fork",
				label: "Fork from here",
				icon: "icon-git-fork",
				disabled: item.historyIndex === undefined,
				onSelect: () => {
					if (item.historyIndex !== undefined) void forkAt(sessionKey, item.historyIndex + 1);
				},
			},
		],
	};
}

function toolOutputText(item: ToolItem): string {
	const result = item.result;
	if (result && typeof result === "object") {
		const record = result as Record<string, unknown>;
		const parts = [record.stdout, record.stderr, record.content].filter(
			(part): part is string => typeof part === "string",
		);
		if (parts.length) return parts.join("\n");
	}
	if (typeof result === "string") return result;
	return JSON.stringify(result ?? item.error ?? null, null, 2);
}

export function toolCardMenu(item: ToolItem): MenuSpec {
	const view = toolView(item);
	return {
		label: `${view.verb} ${view.target || ""}`.trim(),
		items: [
			{
				id: "copy-target",
				label: item.name === "exec" ? "Copy command" : "Copy target",
				icon: "icon-copy",
				disabled: !view.target,
				onSelect: () => copy(item.name === "exec" ? String(item.args.command || view.target) : view.target || ""),
			},
			{
				id: "copy-output",
				label: "Copy output",
				icon: "icon-document",
				disabled: item.status === "running",
				onSelect: () => copy(toolOutputText(item)),
			},
			{
				id: "copy-args",
				label: "Copy arguments (JSON)",
				icon: "icon-code",
				onSelect: () => copy(JSON.stringify(item.args, null, 2)),
			},
		],
	};
}
