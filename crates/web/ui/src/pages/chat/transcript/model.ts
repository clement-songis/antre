// ── Transcript message model ────────────────────────────────
//
// The chat transcript is a list of typed items rendered by Preact
// components. Live WebSocket events and history loading both produce these
// items, so a message looks the same whether it streamed in or was reloaded.

import type { ChannelFooterInfo, DocumentAttachment } from "../../../chat-ui";

export type ToolStatus = "running" | "ok" | "error" | "retry" | "cancelled";

export interface ImageRef {
	dataUrl: string;
	name: string;
}

export interface UserItem {
	kind: "user";
	id: string;
	text: string;
	images: ImageRef[];
	documents: DocumentAttachment[];
	audioSrc?: string;
	channel?: ChannelFooterInfo;
	/** Small print under the message (e.g. which speech-to-text provider). */
	footnote?: string;
	historyIndex?: number;
	createdAt?: number;
}

export interface AssistantUsage {
	model: string;
	provider: string;
	inputTokens: number;
	outputTokens: number;
	cacheReadTokens: number;
	durationMs: number;
}

export interface AssistantItem {
	kind: "assistant";
	id: string;
	text: string;
	reasoning: string;
	/** True while deltas are still arriving. */
	streaming: boolean;
	/** Set once the turn ended: shows the footer and the action bar. */
	final: boolean;
	usage?: AssistantUsage;
	audioSrc?: string;
	audioAutoplay?: boolean;
	audioWarning?: string;
	ttsProvider?: string;
	replyMedium?: string;
	runId?: string;
	historyIndex?: number;
	createdAt?: number;
}

export interface ToolItem {
	kind: "tool";
	id: string;
	toolCallId: string;
	runId?: string;
	name: string;
	args: Record<string, unknown>;
	status: ToolStatus;
	/** Tool output as the server sent it; renderers narrow it per tool. */
	result?: unknown;
	error?: string;
	reasoning?: string;
	executionMode?: string;
	startedAt?: number;
	endedAt?: number;
	historyIndex?: number;
	/** Usage of a turn that ended on this call without any reply text. */
	turnFooter?: TurnFooter;
}

export interface TurnFooter {
	usage?: AssistantUsage;
	replyMedium?: string;
	createdAt?: number;
}

export interface NoticeItem {
	kind: "notice";
	id: string;
	level: "system" | "error";
	text: string;
	/** `text` is already-escaped markdown HTML. */
	html?: boolean;
	className?: string;
	historyIndex?: number;
}

export interface ThinkingItem {
	kind: "thinking";
	id: string;
	text: string;
	/** "retry" shows a provider retry notice instead of reasoning. */
	tone?: "reasoning" | "retry" | "loading";
}

/**
 * A node built imperatively by code not yet ported to components (context
 * card, sandbox progress, error cards). The component only mounts it.
 */
export interface DomItem {
	kind: "dom";
	id: string;
	node: HTMLElement;
}

export type TranscriptItem = UserItem | AssistantItem | ToolItem | NoticeItem | ThinkingItem | DomItem;

export const THINKING_ITEM_ID = "thinkingIndicator";

let nextLocalId = 0;

/** A process-unique id for items that have no server identity. */
export function localItemId(prefix: string): string {
	nextLocalId += 1;
	return `${prefix}-${nextLocalId}`;
}

/** Stable id for a tool card; matches the legacy `tool-<run>-<call>` DOM ids. */
export function toolItemId(toolCallId: string, runId?: string): string {
	return runId ? `tool-${runId}-${toolCallId}` : `tool-${toolCallId}`;
}
