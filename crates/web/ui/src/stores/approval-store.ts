// ── Exec approval store (signal-based) ──────────────────────
//
// Pending approval requests across every session. The server announces a
// request with `exec.approval.requested` and its end, whatever caused it
// (a client, a channel /approve, the timeout), with `exec.approval.resolved`.

import { computed, signal } from "@preact/signals";
import { sendRpc } from "../helpers";

export type ApprovalDecision = "approved" | "denied" | "timeout";

export interface PendingApproval {
	requestId: string;
	command: string;
	sessionKey: string | null;
	/** Unix epoch ms when the server times the request out; 0 when unknown. */
	expiresAtMs: number;
	/** Set while a resolve RPC is in flight. */
	resolving: boolean;
	error: string | null;
}

export interface ApprovalRequestedPayload {
	requestId: string;
	command: string;
	sessionKey?: string | null;
	expiresAtMs?: number | null;
}

export interface ApprovalResolvedPayload {
	requestId: string;
	decision?: ApprovalDecision;
	sessionKey?: string | null;
}

interface PendingRequestView {
	id: string;
	command: string;
	session_key?: string | null;
	expires_at_ms?: number;
}

export const pendingApprovals = signal<PendingApproval[]>([]);

export const pendingApprovalCount = computed(() => pendingApprovals.value.length);

export function approvalsForSession(sessionKey: string): PendingApproval[] {
	return pendingApprovals.value.filter((a) => a.sessionKey === sessionKey || a.sessionKey === null);
}

export function sessionHasPendingApproval(sessionKey: string): boolean {
	return pendingApprovals.value.some((a) => a.sessionKey === sessionKey);
}

function upsert(approval: PendingApproval): void {
	const rest = pendingApprovals.value.filter((a) => a.requestId !== approval.requestId);
	pendingApprovals.value = [...rest, approval];
}

function patch(requestId: string, fields: Partial<PendingApproval>): void {
	pendingApprovals.value = pendingApprovals.value.map((a) => (a.requestId === requestId ? { ...a, ...fields } : a));
}

export function removeApproval(requestId: string): void {
	pendingApprovals.value = pendingApprovals.value.filter((a) => a.requestId !== requestId);
}

export function handleApprovalRequested(payload: ApprovalRequestedPayload): void {
	if (!payload?.requestId) return;
	upsert({
		requestId: payload.requestId,
		command: payload.command || "",
		sessionKey: payload.sessionKey ?? null,
		expiresAtMs: Number(payload.expiresAtMs) || 0,
		resolving: false,
		error: null,
	});
}

export function handleApprovalResolved(payload: ApprovalResolvedPayload): void {
	if (!payload?.requestId) return;
	removeApproval(payload.requestId);
}

/** Replace the store with the server's pending list (after connect). */
export async function refreshPendingApprovals(): Promise<void> {
	const res = await sendRpc<{ requests?: PendingRequestView[] }>("exec.approval.request", {});
	if (!res?.ok) return;
	const requests = res.payload?.requests || [];
	pendingApprovals.value = requests.map((r) => ({
		requestId: r.id,
		command: r.command,
		sessionKey: r.session_key ?? null,
		expiresAtMs: Number(r.expires_at_ms) || 0,
		resolving: false,
		error: null,
	}));
}

export type ApprovalChoice = "once" | "always" | "deny";

/**
 * "always" sends the command back so the server remembers it (persisted
 * across restarts); "once" approves this request only.
 */
export async function resolvePendingApproval(requestId: string, choice: ApprovalChoice): Promise<boolean> {
	const approval = pendingApprovals.value.find((a) => a.requestId === requestId);
	if (!approval || approval.resolving) return false;
	patch(requestId, { resolving: true, error: null });
	const params: Record<string, string> = {
		requestId,
		decision: choice === "deny" ? "denied" : "approved",
	};
	if (choice === "always") params.command = approval.command;
	const res = await sendRpc("exec.approval.resolve", params);
	if (!res?.ok) {
		patch(requestId, { resolving: false, error: res?.error?.message || "Failed to resolve approval" });
		return false;
	}
	removeApproval(requestId);
	return true;
}

export function resetApprovalStore(): void {
	pendingApprovals.value = [];
}
