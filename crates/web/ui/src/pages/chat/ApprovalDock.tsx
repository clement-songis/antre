// ── Pending approvals for the active session ────────────────
//
// Pinned above the composer so a request stays visible however far the
// transcript scrolls. Enter allows once, A always allows, Esc denies the
// first request (when the composer holds no text).

import { useEffect, useState } from "preact/hooks";
import { switchSession } from "../../sessions";
import {
	type ApprovalChoice,
	approvalsForSession,
	type PendingApproval,
	pendingApprovals,
	resolvePendingApproval,
} from "../../stores/approval-store";
import { sessionStore } from "../../stores/session-store";

function secondsLeft(expiresAtMs: number, now: number): number | null {
	if (!expiresAtMs) return null;
	return Math.max(0, Math.ceil((expiresAtMs - now) / 1000));
}

function useNow(active: boolean): number {
	const [now, setNow] = useState(Date.now());
	useEffect(() => {
		if (!active) return;
		const timer = setInterval(() => setNow(Date.now()), 1000);
		return () => clearInterval(timer);
	}, [active]);
	return now;
}

interface ApprovalCardProps {
	approval: PendingApproval;
	now: number;
	primary: boolean;
}

function ApprovalCard({ approval, now, primary }: ApprovalCardProps) {
	const left = secondsLeft(approval.expiresAtMs, now);
	const expired = left === 0;
	const disabled = approval.resolving || expired;
	const choose = (choice: ApprovalChoice) => void resolvePendingApproval(approval.requestId, choice);
	return (
		<section
			className={`msg approval-card${expired ? " approval-expired" : ""}`}
			id={`approval-${approval.requestId}`}
			data-request-id={approval.requestId}
			aria-label="Command approval"
		>
			<div className="approval-label">Command requires approval:</div>
			<code className="approval-cmd">{approval.command}</code>
			<div className="approval-btns">
				<button
					type="button"
					className="approval-btn approval-allow"
					disabled={disabled}
					onClick={() => choose("once")}
					title={primary ? "Allow once (Enter)" : "Allow once"}
				>
					Allow once
				</button>
				<button
					type="button"
					className="approval-btn approval-always"
					disabled={disabled}
					onClick={() => choose("always")}
					title={primary ? "Always allow this exact command (A)" : "Always allow this exact command"}
				>
					Always allow
				</button>
				<button
					type="button"
					className="approval-btn approval-deny"
					disabled={disabled}
					onClick={() => choose("deny")}
					title={primary ? "Deny (Esc)" : "Deny"}
				>
					Deny
				</button>
			</div>
			<div className="approval-countdown">{expired ? "expired" : left === null ? "" : `${left}s`}</div>
			{approval.error ? <div className="approval-status text-[var(--error)]">{approval.error}</div> : null}
		</section>
	);
}

function isTypingTarget(target: EventTarget | null): boolean {
	if (!(target instanceof HTMLElement)) return false;
	if (target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement) {
		return target.value.trim().length > 0;
	}
	return target.isContentEditable;
}

/** Keyboard shortcut handling for the first pending approval. Returns true when handled. */
export function handleApprovalShortcut(e: KeyboardEvent): boolean {
	if (e.defaultPrevented || e.metaKey || e.ctrlKey || e.altKey || e.isComposing) return false;
	const first = approvalsForSession(sessionStore.activeSessionKey.value)[0];
	if (!first || first.resolving) return false;
	if (isTypingTarget(e.target)) return false;
	let choice: ApprovalChoice | null = null;
	if (e.key === "Enter" && !e.shiftKey) choice = "once";
	else if (e.key === "Escape") choice = "deny";
	else if ((e.key === "a" || e.key === "A") && !e.shiftKey) choice = "always";
	if (!choice) return false;
	e.preventDefault();
	e.stopPropagation();
	void resolvePendingApproval(first.requestId, choice);
	return true;
}

export function ApprovalDock() {
	const activeKey = sessionStore.activeSessionKey.value;
	// Read the signal so the dock re-renders on store changes.
	void pendingApprovals.value;
	const approvals = approvalsForSession(activeKey);
	const now = useNow(approvals.length > 0);
	if (approvals.length === 0) return null;
	return (
		<div className="approval-dock flex flex-col gap-2 px-4 pt-2" aria-live="polite">
			{approvals.map((approval, index) => (
				<ApprovalCard key={approval.requestId} approval={approval} now={now} primary={index === 0} />
			))}
		</div>
	);
}

/** Toolbar badge counting approvals waiting in other sessions. */
export function ApprovalsBadge() {
	const activeKey = sessionStore.activeSessionKey.value;
	const elsewhere = pendingApprovals.value.filter((a) => a.sessionKey && a.sessionKey !== activeKey);
	if (elsewhere.length === 0) return null;
	const target = elsewhere[0].sessionKey as string;
	const label = `${elsewhere.length} approval${elsewhere.length === 1 ? "" : "s"} waiting`;
	return (
		<button
			type="button"
			className="approvals-badge text-xs border border-[var(--warn)] text-[var(--warn)] px-2 py-1 rounded-md bg-transparent cursor-pointer inline-flex items-center gap-1"
			title={`${label} in other sessions; click to open`}
			aria-label={label}
			onClick={() => switchSession(target)}
		>
			<span className="icon icon-md icon-lock shrink-0" />
			<span>{elsewhere.length}</span>
		</button>
	);
}
