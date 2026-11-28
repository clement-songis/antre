// ── Unread count in the tab title, "reply ready" toasts ─────

import { effect } from "@preact/signals";
import { t } from "../../i18n";
import { switchSession } from "../../sessions";
import { type Session, sessionStore } from "../../stores/session-store";
import { showToast } from "../../ui";

const TITLE_PREFIX = /^\(\d+\) /;

/** True when a reply finished in the session on screen while the tab was hidden. */
let finishedWhileHidden = false;
let titleObserver: MutationObserver | null = null;

function isUnread(session: Session, activeKey: string): boolean {
	if (session.archived) return false;
	const active = session.key === activeKey;
	return session.localUnread.value || (!active && session.badgeCount.value > (session.lastSeenMessageCount || 0));
}

export function unreadSessionCount(): number {
	const activeKey = sessionStore.activeSessionKey.value;
	return sessionStore.sessions.value.filter((session) => isUnread(session, activeKey)).length;
}

let currentCount = 0;

function applyTitle(): void {
	const base = document.title.replace(TITLE_PREFIX, "");
	const next = currentCount > 0 ? `(${currentCount}) ${base}` : base;
	if (document.title !== next) document.title = next;
}

/** Note a reply that finished; toasts when it belongs to a background session the user ran from here. */
export function noteReplyFinished(sessionKey: string, wasReplying: boolean): void {
	const activeKey = sessionStore.activeSessionKey.value;
	if (sessionKey === activeKey) {
		if (document.hidden) {
			finishedWhileHidden = true;
			currentCount = unreadSessionCount() + 1;
			applyTitle();
		}
		return;
	}
	const session = sessionStore.getByKey(sessionKey);
	// Channel and cron sessions finish on their own schedule; a toast for each would be noise.
	if (!(session && wasReplying) || session.channelBinding || sessionKey.startsWith("cron:")) return;
	showToast(t("chat:notify.finished", { session: session.label || sessionKey }), "info", {
		action: { label: t("chat:notify.open"), onClick: () => switchSession(sessionKey) },
	});
}

let installed = false;

export function installBackgroundNotify(): void {
	if (installed) return;
	installed = true;
	effect(() => {
		currentCount = unreadSessionCount() + (finishedWhileHidden ? 1 : 0);
		applyTitle();
	});
	document.addEventListener("visibilitychange", () => {
		if (document.hidden || !finishedWhileHidden) return;
		finishedWhileHidden = false;
		currentCount = unreadSessionCount();
		applyTitle();
	});
	// Other code rewrites the title (identity, branch); keep the count on it.
	const titleEl = document.querySelector("title");
	if (titleEl) {
		titleObserver = new MutationObserver(() => applyTitle());
		titleObserver.observe(titleEl, { childList: true, characterData: true, subtree: true });
	}
}
