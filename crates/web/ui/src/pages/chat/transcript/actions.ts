// ── Transcript actions ──────────────────────────────────────
//
// The verbs WebSocket handlers, history loading and slash commands use to
// change the transcript. They hide how items are stored and keep the
// streaming bookkeeping (which assistant item is growing) in one place.

import type { ChannelFooterInfo, DocumentAttachment } from "../../../chat-ui";
import type { ChatPayload, ToolResult } from "../../../types/ws-events";
import {
	type AssistantItem,
	type AssistantUsage,
	type ImageRef,
	localItemId,
	type NoticeItem,
	THINKING_ITEM_ID,
	type ThinkingItem,
	type ToolItem,
	type ToolStatus,
	type TurnFooter,
	toolItemId,
} from "./model";
import {
	appendItem,
	assistantTextWithPending,
	flushPendingDeltas,
	getItem,
	queueAssistantDelta,
	removeItem,
	transcriptItems,
	updateItem,
} from "./store";

// ── Thinking indicator ──────────────────────────────────────

export function showThinking(text = "", tone: ThinkingItem["tone"] = "reasoning"): void {
	const existing = getItem(THINKING_ITEM_ID, "thinking");
	if (existing) {
		updateItem(THINKING_ITEM_ID, "thinking", () => ({ text, tone }));
		return;
	}
	appendItem({ kind: "thinking", id: THINKING_ITEM_ID, text, tone });
}

export function hideThinking(): void {
	removeItem(THINKING_ITEM_ID);
}

/** The reasoning text shown live, if any. */
export function currentThinkingText(): string {
	const item = getItem(THINKING_ITEM_ID, "thinking");
	return item && item.tone !== "retry" ? item.text.trim() : "";
}

// ── Streaming assistant text ────────────────────────────────

let streamingId: string | null = null;

export function streamingAssistantId(): string | null {
	return streamingId && getItem(streamingId, "assistant") ? streamingId : null;
}

/** Append a delta to the growing reply, creating it on the first delta. */
export function appendAssistantDelta(delta: string, runId?: string): void {
	let id = streamingAssistantId();
	if (!id) {
		hideThinking();
		id = localItemId("assistant");
		streamingId = id;
		appendItem({
			kind: "assistant",
			id,
			text: "",
			reasoning: "",
			streaming: true,
			final: false,
			runId,
		});
	}
	queueAssistantDelta(id, delta);
}

/**
 * Show text an active run streamed before this client connected. Later
 * deltas continue the same bubble.
 */
export function resumeStreamedText(text: string): void {
	if (streamingAssistantId() || !text.trim()) return;
	appendAssistantDelta(text);
	flushPendingDeltas();
}

/** Text streamed so far for the current reply. */
export function streamedText(): string {
	const id = streamingAssistantId();
	return id ? assistantTextWithPending(id) : "";
}

/**
 * Stop growing the current reply (a tool call started). An empty reply is
 * dropped; otherwise it stays as a settled, non-final message.
 */
export function closeStream(): void {
	flushPendingDeltas();
	const id = streamingAssistantId();
	streamingId = null;
	if (!id) return;
	const item = getItem(id, "assistant");
	if (!item?.text.trim()) {
		removeItem(id);
		return;
	}
	updateItem(id, "assistant", () => ({ streaming: false }));
}

export function resetStream(): void {
	streamingId = null;
}

export interface FinalAssistantFields {
	text: string;
	reasoning?: string;
	usage?: AssistantUsage;
	audioSrc?: string;
	audioAutoplay?: boolean;
	audioWarning?: string;
	replyMedium?: string;
	runId?: string;
	historyIndex?: number;
	createdAt?: number;
}

/**
 * Settle the reply of a finished turn. Reuses the streamed item when there
 * is one so the text does not jump; returns the item id.
 */
export function finalizeAssistant(fields: FinalAssistantFields): string {
	flushPendingDeltas();
	const id = streamingAssistantId();
	streamingId = null;
	const patch: Partial<AssistantItem> = {
		text: fields.text,
		reasoning: fields.reasoning || "",
		streaming: false,
		final: true,
		usage: fields.usage,
		audioSrc: fields.audioSrc,
		audioAutoplay: fields.audioAutoplay,
		audioWarning: fields.audioWarning,
		replyMedium: fields.replyMedium,
		runId: fields.runId,
		historyIndex: fields.historyIndex,
		createdAt: fields.createdAt,
	};
	if (id) {
		updateItem(id, "assistant", () => patch);
		return id;
	}
	const newId = localItemId("assistant");
	appendItem({ kind: "assistant", id: newId, ...(patch as Omit<AssistantItem, "kind" | "id">) });
	return newId;
}

/** Show a silent turn's usage on its last tool card, if it ended on one. */
export function attachTurnFooterToLastTool(footer: TurnFooter): void {
	const items = transcriptItems.value;
	for (let index = items.length - 1; index >= 0; index--) {
		const item = items[index];
		if (item.kind === "user") return;
		if (item.kind === "tool") {
			updateItem(item.id, "tool", () => ({ turnFooter: footer }));
			return;
		}
	}
}

/** Drop the streamed reply (silent reply, or text that only echoed a tool). */
export function discardStream(): void {
	flushPendingDeltas();
	const id = streamingAssistantId();
	streamingId = null;
	if (id) removeItem(id);
}

export function usageFromPayload(p: {
	model?: string;
	provider?: string;
	inputTokens?: number;
	outputTokens?: number;
	cacheReadTokens?: number;
	durationMs?: number;
}): AssistantUsage | undefined {
	if (!p.model) return undefined;
	return {
		model: p.model,
		provider: p.provider || "",
		inputTokens: p.inputTokens || 0,
		outputTokens: p.outputTokens || 0,
		cacheReadTokens: p.cacheReadTokens || 0,
		durationMs: p.durationMs || 0,
	};
}

// ── Tool calls ──────────────────────────────────────────────

export function startTool(p: ChatPayload, reasoning?: string | null): string {
	const id = toolItemId(p.toolCallId || "", p.runId);
	if (getItem(id, "tool")) return id;
	appendItem({
		kind: "tool",
		id,
		toolCallId: p.toolCallId || "",
		runId: p.runId,
		name: p.toolName || "tool",
		args: p.arguments || {},
		status: "running",
		reasoning: reasoning || undefined,
		executionMode: p.executionMode,
		startedAt: Date.now(),
	});
	return id;
}

function toolErrorText(p: ChatPayload): string | undefined {
	if (p.success) return undefined;
	const err = p.error;
	if (!err) return undefined;
	if (typeof err === "string") return err;
	return err.detail || err.message || undefined;
}

/** Record a tool result. An end that beats its start creates the card. */
export function endTool(p: ChatPayload): string {
	const id = toolItemId(p.toolCallId || "", p.runId);
	const status: ToolStatus = p.success ? "ok" : "error";
	const fields: Partial<ToolItem> = {
		status,
		result: p.result as ToolResult | undefined,
		error: toolErrorText(p),
		endedAt: Date.now(),
	};
	if (p.messageIndex !== undefined && p.messageIndex !== null) fields.historyIndex = p.messageIndex;
	if (!updateItem(id, "tool", () => fields)) {
		appendItem({
			kind: "tool",
			id,
			toolCallId: p.toolCallId || "",
			runId: p.runId,
			name: p.toolName || "tool",
			args: p.arguments || {},
			status,
			...fields,
		} as ToolItem);
	}
	return id;
}

/**
 * Settle tool cards still running when the run ends. After an abort or an
 * error their tool never reported back, so they are cancelled, not ok.
 */
export function settleRunningTools(outcome: "finished" | "cancelled"): void {
	const status: ToolStatus = outcome === "cancelled" ? "cancelled" : "ok";
	const now = Date.now();
	transcriptItems.value = transcriptItems.value.map((item) =>
		item.kind === "tool" && item.status === "running" ? { ...item, status, endedAt: now } : item,
	);
}

// ── Users, notices, foreign nodes ───────────────────────────

export interface UserMessageFields {
	text: string;
	images?: ImageRef[];
	documents?: DocumentAttachment[];
	audioSrc?: string;
	channel?: ChannelFooterInfo;
	footnote?: string;
	historyIndex?: number;
	createdAt?: number;
}

export function addUserMessage(fields: UserMessageFields): string {
	const id = localItemId("user");
	appendItem({
		kind: "user",
		id,
		text: fields.text,
		images: fields.images || [],
		documents: fields.documents || [],
		audioSrc: fields.audioSrc,
		channel: fields.channel,
		footnote: fields.footnote,
		historyIndex: fields.historyIndex,
		createdAt: fields.createdAt,
	});
	return id;
}

export function addNotice(level: NoticeItem["level"], text: string, options: Partial<NoticeItem> = {}): string {
	const id = options.id || localItemId("notice");
	appendItem({ kind: "notice", id, level, text, ...options });
	return id;
}

/** Mount an imperatively built element in the transcript; returns its item id. */
export function appendNode(node: HTMLElement): string {
	const id = node.id ? `dom-${node.id}` : localItemId("dom");
	appendItem({ kind: "dom", id, node });
	return id;
}

/** Remove the item holding `node`, if any. */
export function removeNode(node: HTMLElement | null | undefined): void {
	if (!node) return;
	const item = transcriptItems.value.find((entry) => entry.kind === "dom" && entry.node === node);
	if (item) removeItem(item.id);
	else node.remove();
}
