// ── Rewind, regenerate, edit-and-resend ─────────────────────
//
// Built on three server calls: `sessions.rewind_points` (user turns with
// their history index and the runs that followed), `sessions.truncate`
// (drop the conversation after a point) and `checkpoints.restore` (undo the
// file changes of given runs).

import { effect, signal } from "@preact/signals";
import { sendRpc } from "../../helpers";
import { clearHistoryPaginationState, clearSessionHistoryCache, fetchSessions, switchSession } from "../../sessions";
import * as S from "../../state";
import { sessionStore } from "../../stores/session-store";
import { showToast } from "../../ui";
import { dispatchChatMessage } from "./chat-send";
import type { UserItem } from "./transcript/model";
import { transcriptItems } from "./transcript/store";

export interface RewindPoint {
	index: number;
	text: string;
	hasAttachments: boolean;
	runIds: string[];
}

export type RewindScope = "conversation" | "files" | "both";

/** The user turn the rewind dialog is open for; `null` when closed. */
export const rewindTarget = signal<{ sessionKey: string; point: RewindPoint; points: RewindPoint[] } | null>(null);

/** Set when the dialog should list every turn (Esc Esc) instead of one. */
export const rewindPickerOpen = signal<{ sessionKey: string; points: RewindPoint[] } | null>(null);

export async function fetchRewindPoints(sessionKey: string): Promise<RewindPoint[]> {
	const res = await sendRpc<{ points?: RewindPoint[] }>("sessions.rewind_points", { key: sessionKey });
	if (!res?.ok) throw new Error(res?.error?.message || "Failed to load the session's turns");
	return res.payload?.points || [];
}

/**
 * Match a rendered user message to its rewind point. Items sent in this
 * page view have no history index yet, so they are matched by their rank
 * from the end, which history paging cannot shift.
 */
export function pointForUserItem(item: UserItem, points: RewindPoint[]): RewindPoint | null {
	if (item.historyIndex !== undefined) {
		const exact = points.find((point) => point.index === item.historyIndex);
		if (exact) return exact;
	}
	const userItems = transcriptItems.value.filter((entry) => entry.kind === "user");
	const rankFromEnd = userItems.length - 1 - userItems.findIndex((entry) => entry.id === item.id);
	if (rankFromEnd < 0 || rankFromEnd >= points.length) return null;
	return points[points.length - 1 - rankFromEnd];
}

function sessionIsReplying(sessionKey: string): boolean {
	return sessionStore.getByKey(sessionKey)?.replying.value === true;
}

/** Runs at or after `point`: their file changes come after that message. */
export function runIdsFrom(point: RewindPoint, points: RewindPoint[]): string[] {
	return points.filter((entry) => entry.index >= point.index).flatMap((entry) => entry.runIds);
}

async function truncateTo(sessionKey: string, keep: number): Promise<void> {
	const res = await sendRpc("sessions.truncate", { key: sessionKey, keep });
	if (!res?.ok) throw new Error(res?.error?.message || "Rewind failed");
	clearSessionHistoryCache(sessionKey);
	clearHistoryPaginationState(sessionKey);
}

async function restoreFiles(sessionKey: string, runIds: string[]): Promise<{ restored: number; errors: string[] }> {
	if (runIds.length === 0) return { restored: 0, errors: [] };
	const res = await sendRpc<{ restored?: number; errors?: string[] }>("checkpoints.restore", { sessionKey, runIds });
	if (!res?.ok) throw new Error(res?.error?.message || "Restoring files failed");
	return { restored: res.payload?.restored || 0, errors: res.payload?.errors || [] };
}

/** Wait until `sessionKey` is on screen and its history has loaded. */
export function waitForSessionReady(sessionKey: string, timeoutMs = 10_000): Promise<void> {
	return new Promise((resolve, reject) => {
		let stop: (() => void) | null = null;
		const timer = setTimeout(() => {
			stop?.();
			reject(new Error("Timed out opening the session"));
		}, timeoutMs);
		stop = effect(() => {
			const ready = sessionStore.activeSessionKey.value === sessionKey && !sessionStore.switchInProgress.value;
			if (!ready) return;
			clearTimeout(timer);
			queueMicrotask(() => stop?.());
			resolve();
		});
	});
}

/**
 * Rewind to just before `point`. The conversation scope drops that user
 * message and everything after it and puts its text back in the composer.
 */
export async function rewindTo(
	sessionKey: string,
	point: RewindPoint,
	points: RewindPoint[],
	scope: RewindScope,
): Promise<void> {
	if (sessionIsReplying(sessionKey)) throw new Error("Stop the running reply before rewinding");
	const parts: string[] = [];
	if (scope === "files" || scope === "both") {
		const outcome = await restoreFiles(sessionKey, runIdsFrom(point, points));
		parts.push(outcome.restored ? `restored ${outcome.restored} file checkpoint(s)` : "no file changes to restore");
		if (outcome.errors.length) parts.push(`${outcome.errors.length} restore error(s)`);
	}
	if (scope === "conversation" || scope === "both") {
		await truncateTo(sessionKey, point.index);
		fetchSessions();
		switchSession(sessionKey);
		await waitForSessionReady(sessionKey).catch(() => undefined);
		if (S.chatInput && !point.hasAttachments) {
			(S.chatInput as HTMLTextAreaElement).value = point.text;
			S.chatInput.dispatchEvent(new Event("input"));
			S.chatInput.focus();
		}
		parts.push("conversation rewound");
	}
	showToast(`Rewind: ${parts.join(", ")}`, "success");
}

/** Drop the last reply and ask again with the same user message. */
export async function regenerateLastTurn(sessionKey: string): Promise<void> {
	if (sessionIsReplying(sessionKey)) throw new Error("A reply is still running");
	const points = await fetchRewindPoints(sessionKey);
	const last = points[points.length - 1];
	if (!last) throw new Error("Nothing to regenerate yet");
	if (last.hasAttachments) throw new Error("This message has attachments; edit it to resend instead");
	await truncateTo(sessionKey, last.index);
	switchSession(sessionKey);
	await waitForSessionReady(sessionKey);
	await dispatchChatMessage({ sessionKey, text: last.text });
}

/**
 * Edit an earlier message: fork the session just before it, switch to the
 * fork and send the new text there. The original session is untouched.
 */
export async function editAndResend(sessionKey: string, point: RewindPoint, text: string): Promise<string> {
	const res = await sendRpc<{ sessionKey?: string; key?: string }>("sessions.fork", {
		key: sessionKey,
		forkPoint: point.index,
	});
	const forkKey = res?.payload?.sessionKey || res?.payload?.key;
	if (!(res?.ok && forkKey)) throw new Error(res?.error?.message || "Fork failed");
	fetchSessions();
	switchSession(forkKey);
	await waitForSessionReady(forkKey);
	await dispatchChatMessage({ sessionKey: forkKey, text });
	return forkKey;
}
