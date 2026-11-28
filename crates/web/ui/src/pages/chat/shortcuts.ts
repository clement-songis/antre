// ── Global chat keyboard shortcuts ──────────────────────────
//
// Mod is Cmd on macOS and Ctrl elsewhere. Shortcuts that would fight the
// browser (Ctrl+Shift+C opens devtools) use Alt+Shift instead.

import { setSidebarCollapsed } from "../../app-shell";
import { t } from "../../i18n";
import { toggleSessions } from "../../mobile";
import { startNewSession, switchSession } from "../../sessions";
import * as S from "../../state";
import { sessionStore } from "../../stores/session-store";
import { copyToClipboard, showToast } from "../../ui";
import { isMac, shortcutsHelpOpen } from "./shortcut-keys";
import type { AssistantItem } from "./transcript/model";
import { transcriptItems } from "./transcript/store";

const PHONE_WIDTH = 768;

function onChatPage(): boolean {
	return window.location.pathname.startsWith("/chats");
}

export function toggleSidebar(): void {
	if (window.innerWidth < PHONE_WIDTH) {
		toggleSessions();
		return;
	}
	setSidebarCollapsed(!document.body.classList.contains("sidebar-collapsed"));
}

export function focusComposer(): void {
	const input = S.chatInput as HTMLTextAreaElement | null;
	if (!input) return;
	input.focus();
	input.setSelectionRange(input.value.length, input.value.length);
}

/** Session keys in the order the sidebar shows them (pins, filters and folding included). */
function visibleSessionKeys(): string[] {
	return [...document.querySelectorAll<HTMLElement>("#sessionList .session-item[data-session-key]")]
		.filter((row) => row.offsetParent !== null)
		.map((row) => row.dataset.sessionKey as string);
}

export function stepSession(delta: 1 | -1): void {
	const keys = visibleSessionKeys();
	if (keys.length === 0) return;
	const index = keys.indexOf(sessionStore.activeSessionKey.value);
	const next = keys[index < 0 ? 0 : (index + delta + keys.length) % keys.length];
	if (next && next !== sessionStore.activeSessionKey.value) switchSession(next);
}

export function lastReplyText(): string {
	const items = transcriptItems.value;
	for (let i = items.length - 1; i >= 0; i--) {
		const item = items[i];
		if (item.kind === "assistant" && (item as AssistantItem).text.trim()) return (item as AssistantItem).text;
	}
	return "";
}

export function copyLastReply(): void {
	const text = lastReplyText();
	if (!text) {
		showToast(t("chat:shortcuts.nothingToCopy"), "info");
		return;
	}
	void copyToClipboard(text, t("chat:shortcuts.copied"), t("chat:menu.copyFailed"));
}

function matchesMod(e: KeyboardEvent): boolean {
	return isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}

function shortcutFor(e: KeyboardEvent): string | null {
	const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
	if (matchesMod(e) && e.shiftKey && !e.altKey) {
		if (key === "o") return "new-chat";
		if (key === "s") return "sidebar";
	}
	if (matchesMod(e) && !e.shiftKey && !e.altKey && (key === "/" || e.code === "Slash")) return "help";
	if (e.altKey && e.shiftKey && !e.metaKey && !e.ctrlKey) {
		if (key === "ArrowDown") return "next";
		if (key === "ArrowUp") return "prev";
		// Alt changes e.key on macOS layouts, so match the physical key too.
		if (key === "c" || e.code === "KeyC") return "copy";
	}
	if (key === "Escape" && e.shiftKey && !e.altKey && !e.metaKey && !e.ctrlKey) return "focus";
	return null;
}

function run(id: string): void {
	switch (id) {
		case "new-chat":
			startNewSession();
			break;
		case "sidebar":
			toggleSidebar();
			break;
		case "help":
			shortcutsHelpOpen.value = !shortcutsHelpOpen.value;
			break;
		case "next":
			stepSession(1);
			break;
		case "prev":
			stepSession(-1);
			break;
		case "copy":
			copyLastReply();
			break;
		case "focus":
			focusComposer();
			break;
	}
}

const CHAT_ONLY = new Set(["sidebar", "next", "prev", "copy", "focus"]);

function onKeyDown(e: KeyboardEvent): void {
	if (e.defaultPrevented || e.isComposing) return;
	const id = shortcutFor(e);
	if (!id) return;
	if (CHAT_ONLY.has(id) && !onChatPage()) return;
	e.preventDefault();
	e.stopPropagation();
	run(id);
}

let installed = false;

export function installChatShortcuts(): void {
	if (installed) return;
	installed = true;
	window.addEventListener("keydown", onKeyDown, true);
}
