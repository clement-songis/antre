// ── Shared helpers used across ws/ sub-modules ───────────────

import { clearChatEmptyState } from "../chat-ui";
import { renderMarkdown } from "../helpers";
import { appendNode } from "../pages/chat/transcript/actions";
import { transcriptItems } from "../pages/chat/transcript/store";
import { setSessionActiveRunId } from "../sessions";
import * as S from "../state";
import { sessionStore } from "../stores/session-store";

// ── Chat empty-state management ───────────────────────────────

export { clearChatEmptyState } from "../chat-ui";

// ── Thinking UI helpers ───────────────────────────────────────

export function makeThinkingDots(): Element {
	const tpl = S.$<HTMLTemplateElement>("tpl-thinking-dots")!;
	return (tpl.content.cloneNode(true) as DocumentFragment).firstElementChild!;
}

// ── Session helpers ───────────────────────────────────────────

export function updateSessionRunId(sessionKey: string, runId: string | undefined): void {
	if (!runId) return;
	setSessionActiveRunId(sessionKey, runId);
}

export function updateSessionHistoryIndex(sessionKey: string, messageIndex: number | undefined): void {
	const idx = Number(messageIndex);
	if (!Number.isInteger(idx) || idx < 0) return;
	const session = sessionStore.getByKey(sessionKey);
	if (session && idx > session.lastHistoryIndex.value) {
		session.lastHistoryIndex.value = idx;
	}
	if (sessionKey === sessionStore.activeSessionKey.value && idx > S.lastHistoryIndex) {
		S.setLastHistoryIndex(idx);
	}
}

export function moveFirstQueuedToChat(): void {
	const tray = document.getElementById("queuedMessages");
	if (!tray) return;
	const firstQueued = tray.querySelector(".msg.user.queued") as HTMLElement | null;
	if (!firstQueued) return;
	firstQueued.classList.remove("queued");
	const badge = firstQueued.querySelector(".queued-badge");
	if (badge) badge.remove();
	clearChatEmptyState();
	appendNode(firstQueued);
	if (!tray.querySelector(".msg")) tray.classList.add("hidden");
}

// ── Markdown rendering ────────────────────────────────────────

/**
 * Safe wrapper: renderMarkdown uses the `marked` library which HTML-escapes
 * all input by default. No raw user content reaches innerHTML.
 */
export function setSafeMarkdownHtml(el: HTMLElement, text: string): void {
	const rendered = renderMarkdown(text);
	el.textContent = "";
	const wrapper = document.createElement("span");
	wrapper.insertAdjacentHTML("afterbegin", rendered);
	while (wrapper.firstChild) el.appendChild(wrapper.firstChild);
}

export function hasNonWhitespaceContent(text: string | null | undefined): boolean {
	return String(text || "").trim().length > 0;
}

// ── Reasoning dedup ───────────────────────────────────────────

/** Whether a tool card or reply already shows this reasoning text. */
export function isReasoningAlreadyShown(text: string | null | undefined): boolean {
	const normalized = String(text || "").trim();
	if (!normalized) return false;
	return transcriptItems.value.some(
		(item) => (item.kind === "tool" || item.kind === "assistant") && (item.reasoning || "").trim() === normalized,
	);
}

/** Keep the newest reasoning lines in view inside a height-capped element. */
export function showThinkingTail(el: HTMLElement): void {
	el.scrollTop = el.scrollHeight;
	el.classList.toggle("is-overflowing", el.scrollHeight > el.clientHeight + 1);
}
