// ── Transcript root ─────────────────────────────────────────
//
// Mounted once into `#messages` by the chat page. It renders the store; the
// scroll container, the jump-to-latest button and drag-and-drop stay on
// `#messages` itself.

import { effect } from "@preact/signals";
import { render } from "preact";
import { useLayoutEffect } from "preact/hooks";
import { isChatAtBottom, scrollChatToBottom, showNewContentIndicator } from "../../../chat-ui";
import * as S from "../../../state";
import { sessionStore } from "../../../stores/session-store";
import { AssistantMessage, DomSlot, NoticeMessage, ThinkingIndicator, UserMessage } from "./Messages";
import type { ToolItem, TranscriptItem } from "./model";
import { transcriptItems } from "./store";
import { ToolCallCard, ToolGroup } from "./tools/ToolCallCard";
import { PinnedTaskChecklist } from "./tools/tasks";

function lastSettledIndex(items: TranscriptItem[]): number {
	for (let index = items.length - 1; index >= 0; index--) {
		const kind = items[index].kind;
		if (kind !== "thinking" && kind !== "dom" && kind !== "notice") return index;
	}
	return -1;
}

function renderItem(item: TranscriptItem, sessionKey: string, isLast: boolean) {
	switch (item.kind) {
		case "user":
			return <UserMessage key={item.id} item={item} sessionKey={sessionKey} />;
		case "assistant":
			return <AssistantMessage key={item.id} item={item} sessionKey={sessionKey} isLast={isLast} />;
		case "tool":
			return <ToolCallCard key={item.id} item={item} sessionKey={sessionKey} />;
		case "notice":
			return <NoticeMessage key={item.id} item={item} />;
		case "thinking":
			return <ThinkingIndicator key={item.id} item={item} />;
		case "dom":
			return <DomSlot key={item.id} item={item} />;
		default:
			return null;
	}
}

/** Consecutive tool calls (two or more, nothing between them) become one group. */
function groupToolRuns(items: TranscriptItem[]): (TranscriptItem | ToolItem[])[] {
	const out: (TranscriptItem | ToolItem[])[] = [];
	let run: ToolItem[] = [];
	const flush = () => {
		if (run.length >= 2) out.push(run);
		else out.push(...run);
		run = [];
	};
	for (const item of items) {
		if (item.kind === "tool") {
			run.push(item);
			continue;
		}
		flush();
		out.push(item);
	}
	flush();
	return out;
}

// Whether the view sat at the bottom just before the latest store change.
// Measured when the store changes, before the DOM grows, so a large insert
// (a tool result) still keeps a bottom-anchored view anchored.
let wasAtBottom = true;
let stopBottomTracking: (() => void) | null = null;

function trackBottomBeforeRender(): void {
	stopBottomTracking?.();
	stopBottomTracking = effect(() => {
		void transcriptItems.value;
		wasAtBottom = isChatAtBottom();
	});
}

function followNewContent(): void {
	if (S.chatBatchLoading) return;
	if (S.autoScrollMode === "always" || wasAtBottom) {
		scrollChatToBottom(true);
		return;
	}
	showNewContentIndicator();
}

export function Transcript() {
	const items = transcriptItems.value;
	const sessionKey = sessionStore.activeSessionKey.value || S.activeSessionKey;
	const lastIndex = lastSettledIndex(items);
	useLayoutEffect(followNewContent, [items]);
	return (
		<>
			{groupToolRuns(items).map((entry) =>
				Array.isArray(entry) ? (
					<ToolGroup key={`group-${entry[0].id}`} items={entry} sessionKey={sessionKey} />
				) : (
					renderItem(entry, sessionKey, items.indexOf(entry) === lastIndex)
				),
			)}
			<PinnedTaskChecklist />
		</>
	);
}

let mountedRoot: HTMLElement | null = null;

/** Render the transcript into a dedicated root inside the scroll container. */
export function mountTranscript(scrollContainer: HTMLElement): HTMLElement {
	const root = document.createElement("div");
	root.id = "transcriptRoot";
	root.className = "contents";
	scrollContainer.prepend(root);
	trackBottomBeforeRender();
	render(<Transcript />, root);
	mountedRoot = root;
	return root;
}

export function unmountTranscript(): void {
	stopBottomTracking?.();
	stopBottomTracking = null;
	if (!mountedRoot) return;
	render(null, mountedRoot);
	mountedRoot.remove();
	mountedRoot = null;
}
