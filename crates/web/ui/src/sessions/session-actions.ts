// ── Per-session actions for menus ───────────────────────────
//
// The session row menu, the chat title menu and the header buttons all act
// on a session through these functions.

import { signal } from "@preact/signals";
import { sendRpc } from "../helpers";
import { fetchSessions, removeSessionFromClientState, switchSession } from "../sessions";
import { projectStore } from "../stores/project-store";
import { sessionStore } from "../stores/session-store";
import { shareLinkDialog, shareVisibilityDialog, showToast } from "../ui";
import { clearSessionHistoryCache } from "./session-history";

const PINNED_STORAGE_KEY = "moltis-pinned-sessions";
const DELETE_UNDO_MS = 5000;

function loadPinned(): Set<string> {
	try {
		const raw = localStorage.getItem(PINNED_STORAGE_KEY);
		const keys: unknown = raw ? JSON.parse(raw) : [];
		return new Set(Array.isArray(keys) ? keys.filter((k): k is string => typeof k === "string") : []);
	} catch {
		return new Set();
	}
}

/** Sessions pinned to the top of the list, kept in this browser. */
export const pinnedSessionKeys = signal<Set<string>>(loadPinned());

/** Sessions whose deletion waits for its undo window to pass. */
export const pendingDeleteKeys = signal<Set<string>>(new Set());

export function togglePinned(key: string): void {
	const next = new Set(pinnedSessionKeys.value);
	if (next.has(key)) next.delete(key);
	else next.add(key);
	pinnedSessionKeys.value = next;
	try {
		localStorage.setItem(PINNED_STORAGE_KEY, JSON.stringify([...next]));
	} catch {
		// Private mode: pinning lasts for this page only.
	}
}

function errorMessage(res: { error?: { message?: string } | string } | null | undefined, fallback: string): string {
	const err = res?.error;
	if (typeof err === "string") return err || fallback;
	return err?.message || fallback;
}

export async function renameSession(key: string, label: string): Promise<boolean> {
	const trimmed = label.trim();
	if (!trimmed) return false;
	const res = await sendRpc("sessions.patch", { key, label: trimmed });
	if (!res?.ok) {
		showToast(errorMessage(res, "Rename failed"), "error");
		return false;
	}
	const session = sessionStore.getByKey(key);
	if (session) {
		session.label = trimmed;
		session.dataVersion.value++;
	}
	fetchSessions();
	return true;
}

export async function moveSessionToProject(key: string, projectId: string): Promise<void> {
	const res = await sendRpc("sessions.patch", { key, projectId });
	if (!res?.ok) {
		showToast(errorMessage(res, "Move failed"), "error");
		return;
	}
	const session = sessionStore.getByKey(key);
	if (session) {
		session.projectId = projectId;
		session.dataVersion.value++;
	}
	const project = projectStore.getById(projectId);
	showToast(projectId ? `Moved to ${project?.label || projectId}` : "Removed from its project", "success");
	fetchSessions();
}

export async function forkSession(key: string): Promise<void> {
	const res = await sendRpc<{ sessionKey?: string }>("sessions.fork", { key });
	const forkKey = res?.payload?.sessionKey;
	if (!(res?.ok && forkKey)) {
		showToast(errorMessage(res, "Fork failed"), "error");
		return;
	}
	fetchSessions();
	switchSession(forkKey);
}

interface SharePayload {
	path?: string;
	accessKey?: string;
}

function shareUrl(payload: SharePayload): string {
	let url = `${window.location.origin}${payload.path}`;
	if (payload.accessKey) url += `?k=${encodeURIComponent(payload.accessKey)}`;
	return url;
}

export async function shareSession(key: string): Promise<void> {
	const visibility = await shareVisibilityDialog();
	if (!visibility) return;
	const res = await sendRpc<SharePayload>("sessions.share.create", { key, visibility });
	if (!(res?.ok && res.payload?.path)) {
		showToast(errorMessage(res, "Failed to create share link"), "error");
		return;
	}
	const url = shareUrl(res.payload);
	// Sharing appends a boundary notice to the session's history.
	clearSessionHistoryCache(key);
	if (sessionStore.activeSessionKey.value === key) switchSession(key);
	fetchSessions();
	try {
		await navigator.clipboard.writeText(url);
		showToast("Share link copied", "success");
	} catch {
		await shareLinkDialog(url, visibility);
	}
}

export async function setArchived(key: string, archived: boolean): Promise<void> {
	const res = await sendRpc("sessions.patch", { key, archived });
	if (!res?.ok) {
		showToast(errorMessage(res, archived ? "Archive failed" : "Unarchive failed"), "error");
		return;
	}
	const session = sessionStore.getByKey(key);
	if (session) {
		session.archived = archived;
		session.dataVersion.value++;
	}
	fetchSessions();
}

function setPendingDelete(key: string, pending: boolean): void {
	const next = new Set(pendingDeleteKeys.value);
	if (pending) next.add(key);
	else next.delete(key);
	pendingDeleteKeys.value = next;
}

function neighbourKey(key: string): string {
	const sessions = sessionStore.sessions.value.filter((s) => !pendingDeleteKeys.value.has(s.key));
	const index = sessions.findIndex((s) => s.key === key);
	const session = sessions[index];
	if (session?.parentSessionKey) return session.parentSessionKey;
	return sessions[index + 1]?.key || sessions[index - 1]?.key || "main";
}

/**
 * Hide the session at once and delete it on the server after an undo
 * window. Undo brings it back untouched.
 */
export function deleteSessionWithUndo(key: string): void {
	if (key === "main") {
		showToast("The main session cannot be deleted", "error");
		return;
	}
	const wasActive = sessionStore.activeSessionKey.value === key;
	const next = neighbourKey(key);
	setPendingDelete(key, true);
	if (wasActive) switchSession(next);
	let undone = false;
	const label = sessionStore.getByKey(key)?.label || "Session";
	showToast(`Deleted “${label}”`, "info", {
		durationMs: DELETE_UNDO_MS,
		action: {
			label: "Undo",
			onClick: () => {
				undone = true;
				setPendingDelete(key, false);
				if (wasActive) switchSession(key);
			},
		},
	});
	setTimeout(() => {
		if (undone) return;
		sendRpc("sessions.delete", { key }).then((res) => {
			setPendingDelete(key, false);
			if (!res?.ok) {
				showToast(errorMessage(res, "Failed to delete session"), "error");
				fetchSessions();
				return;
			}
			removeSessionFromClientState(key, { nextKey: next });
			fetchSessions();
		});
	}, DELETE_UNDO_MS);
}

/** The session whose name is being edited in the session list. */
export const renamingSessionKey = signal<string | null>(null);

export function canRenameSession(key: string): boolean {
	return key !== "main" && !key.startsWith("cron:");
}
