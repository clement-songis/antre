// ── Session rendering: history messages, welcome card, session list ──

import {
	addQuotaWaitCancelButton,
	chatAddErrorCard,
	type DocumentAttachment,
	highlightAndScroll,
	scrollChatToBottom,
	stripChannelPrefix,
	updateCommandInputUI,
	updateTokenBar,
} from "../chat-ui";
import * as gon from "../gon";
import { parseAgentsListPayload, renderMarkdown, sendRpc } from "../helpers";
import { t } from "../i18n";
import {
	appendNode,
	hideThinking,
	resetStream,
	showThinking,
	usageFromPayload,
} from "../pages/chat/transcript/actions";
import {
	type AssistantItem,
	type ImageRef,
	THINKING_ITEM_ID,
	type ToolItem,
	type TranscriptItem,
	type UserItem,
} from "../pages/chat/transcript/model";
import { getItem, hasItem, removeItem, replaceItems } from "../pages/chat/transcript/store";
import { navigate } from "../router";
import { settingsPath } from "../routes";
import * as S from "../state";
import { modelStore } from "../stores/model-store";
import { sessionStore } from "../stores/session-store";
import type { HistoryMessage } from "../types";

import {
	computeHistoryTailIndex,
	ensureHistoryScrollBinding,
	historyIndexFromMessage,
	syncHistoryState,
} from "./session-history";

// ── Types ────────────────────────────────────────────────────

export interface SearchContext {
	query: string;
	messageIndex: number;
}

interface ToolResultMsg extends HistoryMessage {
	tool_name?: string;
	arguments?: unknown;
	success?: boolean;
	result?: {
		stdout?: string;
		stderr?: string;
		exit_code?: number;
		screenshot?: string;
		document_ref?: string;
		filename?: string;
		mime_type?: string;
		size_bytes?: number;
	};
	error?: string;
	reasoning?: string;
}

interface AssistantMsg extends HistoryMessage {
	content?: string;
	model?: string;
	provider?: string;
	inputTokens?: number;
	outputTokens?: number;
	cacheReadTokens?: number;
	cacheWriteTokens?: number;
	durationMs?: number;
	reasoning?: string;
	audio?: string;
	tts_provider?: string;
	run_id?: string;
	historyIndex?: number;
	requestInputTokens?: number;
	requestOutputTokens?: number;
	requestCacheReadTokens?: number;
	requestCacheWriteTokens?: number;
}

interface UserMsg extends Omit<HistoryMessage, "content"> {
	content?: string | unknown[];
	documents?: Array<{
		display_name?: string;
		stored_filename?: string;
		mime_type?: string;
		size_bytes?: number;
		media_ref?: string;
	}>;
	channel?: {
		channel_type?: string;
		username?: string;
		sender_name?: string;
		message_kind?: string;
	};
	audio?: string;
}

interface AgentInfo {
	id?: string;
	name?: string;
	emoji?: string;
}

interface ExternalAgentInfo {
	installed?: boolean;
	isAcp?: boolean;
}

const installedExternalAgents = new Set<string>();
let externalAgentsLoaded = false;
let externalAgentsLoadingPromise: Promise<void> | null = null;

/** History message with an optional seq field, used for resuming chat sequence counters. */
interface SeqHistoryMessage extends HistoryMessage {
	seq?: number;
	created_at?: number;
}

/** Token usage counters returned by chat.context RPC. */
interface TokenUsage {
	contextWindow?: number;
	/** False when the window is a default rather than the model's real size. */
	contextWindowKnown?: boolean;
	inputTokens?: number;
	outputTokens?: number;
	estimatedNextInputTokens?: number;
	currentInputTokens?: number;
	currentTotal?: number;
}

/** Execution environment info returned by chat.context RPC. */
interface ExecutionInfo {
	mode?: string;
	isRoot?: boolean;
	hostIsRoot?: boolean;
}

/** Payload returned by the chat.context RPC. */
interface ChatContextPayload {
	tokenUsage?: TokenUsage;
	/** A run of this session parked until a provider quota resets. */
	quotaWait?: { provider: string; resetsAt: number } | null;
	supportsTools?: boolean;
	execution?: ExecutionInfo;
}

// ── History → transcript items ───────────────────────────────

/** Extract text and images from a multimodal content array. */
function parseMultimodalContent(blocks: unknown[]): { text: string; images: ImageRef[] } {
	let text = "";
	const images: ImageRef[] = [];
	for (const block of blocks as Array<{ type?: string; text?: string; image_url?: { url?: string } }>) {
		if (block.type === "text") {
			text = block.text || "";
		} else if (block.type === "image_url" && block.image_url?.url) {
			images.push({ dataUrl: block.image_url.url, name: "image" });
		}
	}
	return { text, images };
}

function historyMediaSrc(sessionKey: string, path: string): string {
	const filename = path.split("/").pop() || "";
	return `/api/sessions/${encodeURIComponent(sessionKey)}/media/${encodeURIComponent(filename)}`;
}

/** Item ids derived from the history index stay stable across re-renders. */
function historyItemId(kind: string, msg: HistoryMessage, position: number): string {
	const idx = historyIndexFromMessage(msg);
	return idx === null ? `h-${kind}-p${position}` : `h-${kind}-${idx}`;
}

function userItemFromHistory(msg: UserMsg, sessionKey: string, position: number): UserItem {
	let text = "";
	let images: ImageRef[] = [];
	if (Array.isArray(msg.content)) {
		const parsed = parseMultimodalContent(msg.content);
		text = parsed.text;
		images = parsed.images;
	} else {
		text = (msg.content as string) || "";
	}
	if (msg.channel) text = stripChannelPrefix(text);
	const documents: DocumentAttachment[] = (msg.documents || [])
		.map((doc) => {
			const storedName = doc.stored_filename || doc.media_ref?.split("/").pop() || "";
			return {
				display_name: doc.display_name || storedName,
				stored_filename: storedName,
				mime_type: doc.mime_type || "",
				size_bytes: doc.size_bytes,
			};
		})
		.filter((doc) => doc.stored_filename);
	return {
		kind: "user",
		id: historyItemId("user", msg, position),
		text,
		images,
		documents,
		audioSrc: msg.audio ? historyMediaSrc(sessionKey, msg.audio) : undefined,
		channel: msg.channel,
		historyIndex: historyIndexFromMessage(msg) ?? undefined,
		createdAt: (msg as SeqHistoryMessage).created_at,
	};
}

function assistantItemFromHistory(msg: AssistantMsg, sessionKey: string, position: number): AssistantItem {
	return {
		kind: "assistant",
		id: historyItemId("assistant", msg, position),
		text: msg.content || "",
		reasoning: msg.reasoning || "",
		streaming: false,
		final: true,
		usage: usageFromPayload(msg),
		audioSrc: msg.audio ? historyMediaSrc(sessionKey, msg.audio) : undefined,
		ttsProvider: msg.tts_provider,
		runId: msg.run_id || undefined,
		historyIndex: historyIndexFromMessage(msg) ?? undefined,
		createdAt: (msg as SeqHistoryMessage).created_at,
	};
}

function toolItemFromHistory(msg: ToolResultMsg, position: number): ToolItem {
	const toolCallId = msg.tool_call_id || `p${position}`;
	return {
		kind: "tool",
		id: historyItemId("tool", msg, position),
		toolCallId,
		runId: msg.run_id,
		name: msg.tool_name || "tool",
		args: (msg.arguments && typeof msg.arguments === "object" ? msg.arguments : {}) as Record<string, unknown>,
		status: msg.success ? "ok" : "error",
		result: msg.result,
		error: msg.success ? undefined : msg.error,
		reasoning: msg.reasoning,
		historyIndex: historyIndexFromMessage(msg) ?? undefined,
	};
}

export function historyToItems(history: HistoryMessage[], sessionKey: string): TranscriptItem[] {
	const items: TranscriptItem[] = [];
	history.forEach((msg, position) => {
		if (msg.role === "user") items.push(userItemFromHistory(msg as UserMsg, sessionKey, position));
		else if (msg.role === "assistant") items.push(assistantItemFromHistory(msg as AssistantMsg, sessionKey, position));
		else if (msg.role === "tool_result") items.push(toolItemFromHistory(msg as ToolResultMsg, position));
		else if (msg.role === "notice") {
			items.push({
				kind: "notice",
				id: historyItemId("notice", msg, position),
				level: "system",
				text: renderMarkdown(typeof msg.content === "string" ? msg.content : ""),
				html: true,
				historyIndex: historyIndexFromMessage(msg) ?? undefined,
			});
		}
	});
	return items;
}

function applyHistoryTokenUsage(history: HistoryMessage[]): void {
	S.setSessionTokens({ input: 0, output: 0 });
	S.setSessionCurrentInputTokens(0);
	S.setSessionCurrentContextTokens(0);
	for (const raw of history) {
		if (raw.role !== "assistant") continue;
		const msg = raw as AssistantMsg;
		if (msg.inputTokens || msg.outputTokens) {
			S.sessionTokens.input += msg.inputTokens || 0;
			S.sessionTokens.output += msg.outputTokens || 0;
		}
		if (msg.requestInputTokens !== undefined && msg.requestInputTokens !== null) {
			S.setSessionCurrentInputTokens(msg.requestInputTokens || 0);
		} else if (msg.inputTokens || msg.outputTokens) {
			S.setSessionCurrentInputTokens(msg.inputTokens || 0);
		}
		S.setSessionCurrentContextTokens(
			(msg.requestInputTokens ?? msg.inputTokens ?? 0) +
				(msg.requestOutputTokens ?? msg.outputTokens ?? 0) +
				(msg.requestCacheReadTokens ?? msg.cacheReadTokens ?? 0) +
				(msg.requestCacheWriteTokens ?? msg.cacheWriteTokens ?? 0),
		);
	}
}

/** Rendered elements of the given history entries, in order (null when not rendered). */
function historyElements(history: HistoryMessage[]): (HTMLElement | null)[] {
	return history.map((msg) => {
		const idx = historyIndexFromMessage(msg);
		if (idx === null || !S.chatMsgBox) return null;
		return S.chatMsgBox.querySelector<HTMLElement>(`[data-history-index="${idx}"]`);
	});
}

export function postHistoryLoadActions(
	key: string,
	searchContext: SearchContext | null,
	msgEls: (HTMLElement | null)[],
	thinkingText: string | null,
	skipAutoScroll: boolean,
): void {
	sendRpc("chat.context", {}).then((ctxRes) => {
		if (ctxRes?.ok && ctxRes.payload) {
			const p = ctxRes.payload as ChatContextPayload;
			if (p.tokenUsage) {
				const tu = p.tokenUsage;
				S.setSessionContextWindow(tu.contextWindow || 0);
				S.setSessionContextWindowKnown(tu.contextWindowKnown !== false);
				S.setSessionTokens({
					input: tu.inputTokens || 0,
					output: tu.outputTokens || 0,
				});
				S.setSessionCurrentInputTokens(tu.estimatedNextInputTokens || tu.currentInputTokens || tu.inputTokens || 0);
				S.setSessionCurrentContextTokens(tu.currentTotal || tu.estimatedNextInputTokens || tu.currentInputTokens || 0);
			}
			S.setSessionToolsEnabled(p.supportsTools !== false);
			if (p.quotaWait && key === sessionStore.activeSessionKey.value) {
				chatAddErrorCard({
					icon: "\u23F3",
					title: t("errors:chat.quotaWait.title", "Waiting for quota"),
					detail: t("errors:chat.quotaWait.detail", "Resuming automatically when the quota resets."),
					provider: p.quotaWait.provider,
					resetsAt: p.quotaWait.resetsAt,
				});
				addQuotaWaitCancelButton(key);
			}
			const execution = p.execution || {};
			const mode = execution.mode === "sandbox" ? "sandbox" : "host";
			const hostIsRoot = execution.hostIsRoot === true;
			let isRoot = execution.isRoot;
			if (typeof isRoot !== "boolean") {
				isRoot = mode === "sandbox" ? true : hostIsRoot;
			}
			S.setHostExecIsRoot(hostIsRoot);
			S.setSessionExecMode(mode);
			S.setSessionExecPromptSymbol(isRoot ? "#" : "$");
		}
		updateCommandInputUI();
		updateTokenBar();
	});
	updateTokenBar();

	if (!skipAutoScroll && searchContext?.query && S.chatMsgBox) {
		highlightAndScroll(msgEls, searchContext.messageIndex, searchContext.query);
	} else if (!skipAutoScroll) {
		scrollChatToBottom(true);
	}

	const session = sessionStore.getByKey(key);
	if (session?.replying.value && S.chatMsgBox) {
		showThinking(thinkingText || "");
		if (!skipAutoScroll) scrollChatToBottom(true);
	}
}

/** No-op -- the Preact SessionHeader component auto-updates from signals. */
export function updateChatSessionHeader(): void {
	// Retained for backward compat call sites; Preact handles rendering.
}

export function renderWelcomeAgentPicker(
	card: HTMLElement,
	activeAgentId: string,
	onActiveAgentResolved: (agent: AgentInfo | null) => void,
): void {
	const container = card.querySelector("[data-welcome-agents]") as HTMLElement | null;
	if (!container) return;

	sendRpc("agents.list", {}).then((res) => {
		if (!card.isConnected) return;
		if (!res?.ok) {
			container.classList.add("hidden");
			return;
		}
		const parsed = parseAgentsListPayload(res.payload as Parameters<typeof parseAgentsListPayload>[0]);
		const agents = (parsed.agents || []) as AgentInfo[];
		const defaultId = (parsed.defaultId || "main") as string;
		const effectiveActive = activeAgentId || defaultId;

		container.textContent = "";
		container.classList.remove("hidden");
		container.classList.add("flex");

		let activeAgent: AgentInfo | null = null;
		for (const agent of agents) {
			if (!agent?.id) continue;
			if (agent.id === effectiveActive) activeAgent = agent;
			const chip = document.createElement("button");
			chip.type = "button";
			chip.className = agent.id === effectiveActive ? "provider-btn" : "provider-btn provider-btn-secondary";
			chip.style.fontSize = "0.7rem";
			chip.style.padding = "3px 8px";
			const labelPrefix = agent.emoji ? `${agent.emoji} ` : "";
			chip.textContent = `${labelPrefix}${agent.name || agent.id}`;
			chip.addEventListener("click", () => {
				const key = sessionStore.activeSessionKey.value || S.activeSessionKey || "main";
				sendRpc("agents.set_session", { session_key: key, agent_id: agent.id }).then((setRes) => {
					if (!setRes?.ok) return;
					const live = sessionStore.getByKey(key);
					if (live) {
						live.agent_id = agent.id || "";
						live.dataVersion.value++;
					}
					// Lazy import to avoid circular dependency with sessions.ts
					void import("../sessions").then(({ fetchSessions }) => fetchSessions());
					if (hasItem("dom-welcomeCard")) {
						removeItem("dom-welcomeCard");
						showWelcomeCard();
					}
				});
			});
			container.appendChild(chip);
		}

		const hatchBtn = document.createElement("button");
		hatchBtn.type = "button";
		hatchBtn.className = "provider-btn provider-btn-secondary";
		hatchBtn.style.fontSize = "0.7rem";
		hatchBtn.style.padding = "3px 8px";
		hatchBtn.textContent = "\u{1F95A} Hatch a new agent";
		hatchBtn.addEventListener("click", () => {
			navigate(settingsPath("agents/new"));
		});
		container.appendChild(hatchBtn);

		onActiveAgentResolved(activeAgent);
	});
}

function hasInstalledExternalAgent(): boolean {
	if (externalAgentsLoaded) return installedExternalAgents.size > 0;
	if (!externalAgentsLoadingPromise) {
		externalAgentsLoadingPromise = refreshExternalAgentAvailability().finally(() => {
			externalAgentsLoadingPromise = null;
		});
	}
	return false;
}

function refreshExternalAgentAvailability(): Promise<void> {
	return sendRpc("external_agents.list", {})
		.then((res) => {
			if (!res?.ok) return;
			installedExternalAgents.clear();
			const agents = Array.isArray(res.payload) ? (res.payload as ExternalAgentInfo[]) : [];
			for (const agent of agents) {
				if (agent?.installed) installedExternalAgents.add(String(agent.isAcp ? "acp" : "external"));
			}
			externalAgentsLoaded = true;
			refreshWelcomeCardIfNeeded();
		})
		.catch(() => {});
}

function hasChatBackend(): boolean {
	return modelStore.models.value.length > 0 || hasInstalledExternalAgent();
}

function showWelcomeCard(): void {
	if (!S.chatMsgBox) return;
	S.chatMsgBox.classList.add("chat-messages-empty");

	if (!hasChatBackend()) {
		const noProvTpl = S.$<HTMLTemplateElement>("tpl-no-providers-card");
		if (!noProvTpl) return;
		const noProvCard = (noProvTpl.content.cloneNode(true) as DocumentFragment).firstElementChild as HTMLElement;
		appendNode(noProvCard);
		S.chatMsgBox.classList.add("chat-messages-empty");
		return;
	}

	const tpl = S.$<HTMLTemplateElement>("tpl-welcome-card");
	if (!tpl) return;
	const card = (tpl.content.cloneNode(true) as DocumentFragment).firstElementChild as HTMLElement;
	const identity = gon.get("identity");
	const userName = identity?.user_name;
	const botName = identity?.name || "moltis";
	const botEmoji = identity?.emoji || "";

	const greetingEl = card.querySelector("[data-welcome-greeting]") as HTMLElement | null;
	if (greetingEl) greetingEl.textContent = userName ? `Hello, ${userName}!` : "Hello!";
	const emojiEl = card.querySelector("[data-welcome-emoji]") as HTMLElement | null;
	if (emojiEl) emojiEl.textContent = botEmoji;
	const nameEl = card.querySelector("[data-welcome-bot-name]") as HTMLElement | null;
	if (nameEl) nameEl.textContent = botName;
	const activeAgentId = sessionStore.activeSession.value?.agent_id || "main";
	renderWelcomeAgentPicker(card, activeAgentId, (activeAgent) => {
		if (!activeAgent) return;
		if (emojiEl) emojiEl.textContent = activeAgent.emoji || "";
		if (nameEl) nameEl.textContent = activeAgent.name || botName;
	});

	appendNode(card);
	S.chatMsgBox.classList.add("chat-messages-empty");
}

export function refreshWelcomeCardIfNeeded(): void {
	if (!S.chatMsgBox) return;
	const chatBackendAvailable = hasChatBackend();
	if (chatBackendAvailable && hasItem("dom-noProvidersCard")) {
		removeItem("dom-noProvidersCard");
		showWelcomeCard();
	} else if (!chatBackendAvailable && hasItem("dom-welcomeCard")) {
		removeItem("dom-welcomeCard");
		showWelcomeCard();
	}
}

export function showSessionLoadIndicator(): void {
	if (!S.chatMsgBox) return;
	showThinking("Loading session\u2026", "loading");
}

export function hideSessionLoadIndicator(): void {
	const item = getItem(THINKING_ITEM_ID, "thinking");
	if (item?.tone === "loading") hideThinking();
}

export function renderHistory(
	key: string,
	history: HistoryMessage[],
	searchContext: SearchContext | null,
	thinkingText: string | null,
	totalCountHint: number | null,
	skipAutoScroll: boolean,
): void {
	ensureHistoryScrollBinding();
	S.chatMsgBox?.classList.remove("chat-messages-empty");
	resetStream();
	applyHistoryTokenUsage(history);
	S.setChatBatchLoading(true);
	const items = historyToItems(history, key);
	for (const error of sessionStore.getByKey(key)?.sendErrors.value || []) {
		items.push({
			kind: "notice",
			id: `send-error-${items.length}`,
			level: "error",
			text: error,
			className: "chat-send-error",
		});
	}
	replaceItems(items);
	S.setChatBatchLoading(false);
	const historyTailIndex = computeHistoryTailIndex(history);
	syncHistoryState(key, history, historyTailIndex, totalCountHint);

	let maxSeq = 0;
	for (const hm of history) {
		if (hm.role === "user" && ((hm as SeqHistoryMessage).seq as number) > maxSeq) {
			maxSeq = (hm as SeqHistoryMessage).seq as number;
		}
	}
	S.setChatSeq(maxSeq);
	if (history.length === 0) showWelcomeCard();
	// Elements exist once Preact has rendered the new items.
	requestAnimationFrame(() => {
		postHistoryLoadActions(key, searchContext, historyElements(history), thinkingText, skipAutoScroll === true);
	});
}
