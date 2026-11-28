// ── Chat event handler functions ──────────────────────────────

import {
	addQuotaWaitCancelButton,
	type ChannelFooterInfo,
	chatAddErrorCard,
	chatAddErrorMsg,
	chatAddMsg,
	setComposerStopButton,
	updateTokenBar,
} from "../chat-ui";
import { localizeStructuredError, renderMarkdown } from "../helpers";
import { t } from "../i18n";
import { maybeRefreshFullContext, renderCompactCard } from "../pages/ChatPage";
import { noteReplyFinished } from "../pages/chat/background-notify";
import { clearQueuedMessages, handleQueueItemRemoved } from "../pages/chat/queue";
import {
	addNotice,
	addUserMessage,
	appendAssistantDelta,
	attachTurnFooterToLastTool,
	closeStream,
	currentThinkingText,
	discardStream,
	endTool,
	finalizeAssistant,
	hideThinking,
	settleRunningTools,
	showThinking,
	startTool,
	streamedText,
	streamingAssistantId,
	usageFromPayload,
} from "../pages/chat/transcript/actions";
import { clearTranscript, removeItem } from "../pages/chat/transcript/store";
import { currentPrefix } from "../router";
import {
	bumpSessionCount,
	cacheSessionHistoryMessage,
	clearSessionHistoryCache,
	fetchSessions,
	markSessionLocallyCleared,
	setSessionActiveRunId,
	setSessionReplying,
	setSessionUnread,
} from "../sessions";
import * as S from "../state";
import { markSessionRunStateChanged, sessionStore } from "../stores/session-store";
import type { AbortedPartialState, ChatPayload, CompactPayload } from "../types/ws-events";
import {
	clearChatEmptyState,
	hasNonWhitespaceContent,
	isReasoningAlreadyShown,
	moveFirstQueuedToChat,
	updateSessionHistoryIndex,
	updateSessionRunId,
} from "./shared";
import { isPureToolOutputEcho, stripChannelPrefix } from "./tool-helpers";

export type ChatHandler = (p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string) => void;

// ── Individual chat event handlers ────────────────────────────

function handleChatThinking(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	updateSessionRunId(eventSession, p.runId);
	setSessionReplying(eventSession, true);
	if (!(isActive && isChatPage)) return;
	setComposerStopButton(true, eventSession);
	clearChatEmptyState();
	showThinking("");
}

function handleChatThinkingText(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	updateSessionRunId(eventSession, p.runId);
	setSessionReplying(eventSession, true);
	if (!(isActive && isChatPage)) return;
	setComposerStopButton(true, eventSession);
	// Reasoning after visible text belongs to the next step: keep the text.
	if (streamingAssistantId()) closeStream();
	showThinking(p.text || "");
}

function handleChatThinkingDone(_p: ChatPayload, isActive: boolean, isChatPage: boolean): void {
	// Don't remove the thinking indicator here. It will be removed by either:
	// - handleChatDelta (when text starts streaming)
	// - handleChatToolCallStart (which preserves thinking text as a disclosure)
	// - handleChatFinal / handleChatError (cleanup)
	// This keeps the thinking text visible until we know whether to preserve it.
	void (isActive && isChatPage);
}

function handleChatVoicePending(_p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	markSessionRunStateChanged(eventSession);
	// Update per-session signal
	const session = sessionStore.getByKey(eventSession);
	if (session) session.voicePending.value = true;
	if (!(isActive && isChatPage)) return;
	// Dual-write to global state for backward compat
	S.setVoicePending(true);
	// Keep the existing thinking dots visible -- no separate voice indicator.
}

function handleChatToolCallStart(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	updateSessionRunId(eventSession, p.runId);
	const session = sessionStore.getByKey(eventSession);
	if (session) session.streamText.value = "";
	if (!(isActive && isChatPage)) return;
	setComposerStopButton(true, eventSession);
	// The reasoning that led to this call stays with its card.
	const reasoning = currentThinkingText();
	hideThinking();
	closeStream();
	clearChatEmptyState();
	startTool(p, reasoning);
}

function handleChatToolCallEnd(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	updateSessionRunId(eventSession, p.runId);
	// Always bump badge -- the server persists both the hidden assistant
	// tool-call frame and the visible tool_result for each completed call.
	bumpSessionCount(eventSession, 2);
	let toolHistoryIndex: number | undefined | null = p.messageIndex;
	if (toolHistoryIndex === undefined || toolHistoryIndex === null) {
		const toolSession = sessionStore.getByKey(eventSession);
		if (toolSession && typeof toolSession.messageCount === "number" && toolSession.messageCount > 0) {
			toolHistoryIndex = toolSession.messageCount - 1;
		}
	}
	cacheSessionHistoryMessage(
		eventSession,
		{
			role: "tool_result",
			tool_call_id: p.toolCallId || "",
			tool_name: p.toolName || "",
			arguments: p.arguments || undefined,
			success: p.success === true,
			result: p.result || null,
			error: p.error?.detail || p.error?.message || (typeof p.error === "string" ? String(p.error) : null),
			created_at: Date.now(),
		},
		toolHistoryIndex as number | undefined,
	);
	updateSessionHistoryIndex(eventSession, toolHistoryIndex as number | undefined);
	const out = (p.result?.stdout || "").replace(/\n+$/, "");
	const toolSession = sessionStore.getByKey(eventSession);
	if (toolSession) toolSession.lastToolOutput.value = out;
	if (!(isActive && isChatPage)) return;
	S.setLastToolOutput(out);
	endTool(p);
}

function handleChatChannelUser(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	// Always bump the badge so the total message count stays accurate,
	// even when the user is not on the chat page (e.g. Telegram messages).
	bumpSessionCount(eventSession, 1);
	const cachedAudio = p.channel?.audio_filename;
	cacheSessionHistoryMessage(
		eventSession,
		{
			role: "user",
			content: p.text || "",
			channel: p.channel || null,
			audio: cachedAudio,
			created_at: Date.now(),
		},
		p.messageIndex,
	);
	if (!isActive) {
		setSessionUnread(eventSession, true);
	}
	if (!(isChatPage && isActive)) {
		updateSessionHistoryIndex(eventSession, p.messageIndex);
		return;
	}
	const chanSession = sessionStore.getByKey(eventSession);
	const lastIdx = chanSession ? chanSession.lastHistoryIndex.value : S.lastHistoryIndex;
	if (!(p.messageIndex !== undefined && p.messageIndex <= lastIdx)) {
		const audioFilename = p.channel?.audio_filename;
		addUserMessage({
			text: stripChannelPrefix(p.text || ""),
			audioSrc: audioFilename
				? `/api/sessions/${encodeURIComponent(eventSession)}/media/${encodeURIComponent(audioFilename)}`
				: undefined,
			channel: (p.channel as ChannelFooterInfo | undefined) || undefined,
			historyIndex: p.messageIndex,
		});
	}
	updateSessionHistoryIndex(eventSession, p.messageIndex);
}

// Handle user messages broadcast by the backend after persisting a message
// sent via the GraphQL API, mobile app, or any non-web-UI client.
// The originating web client already rendered the message optimistically,
// so we skip rendering when the broadcast's seq matches a seq this client
// has already sent (seq <= S.chatSeq).
function handleChatUserMessage(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	// Suppress the echo for the originating client.
	if (p.seq !== undefined && p.seq !== null && p.seq <= S.chatSeq) return;

	bumpSessionCount(eventSession, 1);
	cacheSessionHistoryMessage(
		eventSession,
		{
			role: "user",
			content: p.text || "",
			created_at: Date.now(),
		},
		p.messageIndex,
	);
	if (!isActive) {
		setSessionUnread(eventSession, true);
	}
	if (!(isChatPage && isActive)) return;
	addUserMessage({ text: p.text || "", historyIndex: p.messageIndex });
}

function handleChatDelta(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	updateSessionRunId(eventSession, p.runId);
	if (!p.text) return;
	const session = sessionStore.getByKey(eventSession);
	if (session) session.streamText.value += p.text;
	if (!(isActive && isChatPage)) return;
	// When voice is pending, accumulate text silently without rendering.
	if (S.voicePending) {
		S.setStreamText(S.streamText + p.text);
		return;
	}
	// Some providers emit newlines between thinking and content; starting a
	// reply on them would leave an empty bubble if a tool call follows.
	if (!(streamingAssistantId() || p.text.trim())) return;
	clearChatEmptyState();
	appendAssistantDelta(p.text, p.runId);
}

function mediaSrc(sessionKey: string, path: string): string {
	const filename = path.split("/").pop() || "";
	return `/api/sessions/${encodeURIComponent(sessionKey)}/media/${encodeURIComponent(filename)}`;
}

function cacheFinalMessage(eventSession: string, p: ChatPayload, finalText: string): void {
	cacheSessionHistoryMessage(
		eventSession,
		{
			role: "assistant",
			content: finalText,
			model: p.model || "",
			provider: p.provider || "",
			inputTokens: p.inputTokens || 0,
			outputTokens: p.outputTokens || 0,
			cacheReadTokens: p.cacheReadTokens || 0,
			cacheWriteTokens: p.cacheWriteTokens || 0,
			durationMs: p.durationMs || 0,
			requestInputTokens: p.requestInputTokens,
			requestOutputTokens: p.requestOutputTokens,
			requestCacheReadTokens: p.requestCacheReadTokens,
			requestCacheWriteTokens: p.requestCacheWriteTokens,
			reasoning: p.reasoning || undefined,
			audio: p.audio || undefined,
			run_id: p.runId || undefined,
			created_at: Date.now(),
		},
		p.messageIndex,
	);
}

function applyFinalTokenUsage(p: ChatPayload): void {
	if (p.inputTokens || p.outputTokens) {
		S.sessionTokens.input += p.inputTokens || 0;
		S.sessionTokens.output += p.outputTokens || 0;
	}
	if (p.requestInputTokens !== undefined && p.requestInputTokens !== null) {
		S.setSessionCurrentInputTokens(p.requestInputTokens || 0);
	} else if (p.inputTokens || p.outputTokens) {
		S.setSessionCurrentInputTokens(p.inputTokens || 0);
	}
	S.setSessionCurrentContextTokens(
		(p.requestInputTokens ?? p.inputTokens ?? 0) +
			(p.requestOutputTokens ?? p.outputTokens ?? 0) +
			(p.requestCacheReadTokens ?? p.cacheReadTokens ?? 0) +
			(p.requestCacheWriteTokens ?? p.cacheWriteTokens ?? 0),
	);
	updateTokenBar();
}

/** Put the finished reply in the transcript. */
function renderFinalReply(p: ChatPayload, eventSession: string): void {
	const finalText = String(p.text || "");
	const sessionKey = p.sessionKey || eventSession;
	const reasoning = p.reasoning && !isReasoningAlreadyShown(p.reasoning) ? p.reasoning : "";
	const voiceReply = p.replyMedium === "voice" && hasNonWhitespaceContent(finalText);
	const common = {
		reasoning,
		usage: usageFromPayload(p),
		audioWarning: p.audioWarning || undefined,
		replyMedium: p.replyMedium,
		runId: p.runId,
		historyIndex: p.messageIndex,
		createdAt: Date.now(),
	};
	if (voiceReply) {
		finalizeAssistant({
			...common,
			// The audio player replaces streamed text; the text stays below it
			// only when voice was pending (nothing was shown yet).
			text: S.voicePending || !p.audio ? finalText : "",
			audioSrc: p.audio ? mediaSrc(sessionKey, p.audio) : undefined,
			audioAutoplay: true,
		});
		return;
	}
	const hasText = hasNonWhitespaceContent(finalText);
	if (hasText && !isPureToolOutputEcho(finalText, S.lastToolOutput)) {
		finalizeAssistant({ ...common, text: finalText });
		return;
	}
	if (reasoning) {
		finalizeAssistant({ ...common, text: "" });
		return;
	}
	// Silent reply, or text that only repeated the last tool output.
	discardStream();
	attachTurnFooterToLastTool({ usage: common.usage, replyMedium: p.replyMedium, createdAt: common.createdAt });
}

function handleChatFinal(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	updateSessionRunId(eventSession, p.runId);
	// Always bump badge -- the server persists the final assistant message.
	bumpSessionCount(eventSession, 1);
	const finalText = String(p.text || "");
	const hasVisibleFinal =
		hasNonWhitespaceContent(finalText) ||
		hasNonWhitespaceContent(p.reasoning || "") ||
		hasNonWhitespaceContent(p.audio || "");
	if (hasVisibleFinal) cacheFinalMessage(eventSession, p, finalText);
	// Compare against the per-session history index so cross-session
	// events aren't wrongly skipped by another session's index.
	const evtSession = sessionStore.getByKey(eventSession);
	const lastIdx = evtSession ? evtSession.lastHistoryIndex.value : S.lastHistoryIndex;
	if (p.messageIndex !== undefined && p.messageIndex <= lastIdx) {
		setSessionReplying(eventSession, false);
		setSessionActiveRunId(eventSession, null);
		if (isActive && isChatPage) setComposerStopButton(false);
		return;
	}
	updateSessionHistoryIndex(eventSession, p.messageIndex);
	const wasReplying = evtSession?.replying.value === true;
	setSessionReplying(eventSession, false);
	setSessionActiveRunId(eventSession, null);
	if (!isActive) setSessionUnread(eventSession, true);
	if (hasVisibleFinal) noteReplyFinished(eventSession, wasReplying);
	if (!(isActive && isChatPage)) {
		S.setVoicePending(false);
		return;
	}
	setComposerStopButton(false);
	hideThinking();
	settleRunningTools("finished");
	renderFinalReply(p, eventSession);
	applyFinalTokenUsage(p);
	if (evtSession) evtSession.resetStreamState();
	S.setStreamEl(null);
	S.setStreamText("");
	S.setLastToolOutput("");
	S.setVoicePending(false);
	maybeRefreshFullContext();
	// Move the next queued message from the tray only after the reply is in
	// place, so the order stays user -> reply -> next user -> next reply.
	moveFirstQueuedToChat();
}

// ── Compact handling ──────────────────────────────────────────

// Shared debounce so the auto-compact path (which broadcasts both
// `chat.compact done` from within ChatService::compact AND a wrapping
// `auto_compact done` from the send() caller) renders the card exactly
// once. Whichever event arrives first claims the render; the other is
// a no-op within the debounce window.
const COMPACT_CARD_DEBOUNCE_MS = 500;
const lastCompactCardAt: Map<string, number> = new Map();

// Per-session reference to the "Compacting conversation..." status message
// appended on `auto_compact start`. Tracked explicitly (not via
// `lastChild`) because `send()`'s pre-emptive auto-compact path
// interleaves `chat.compact done` between `auto_compact start` and
// `auto_compact done`, which means the old "remove lastChild" pattern
// would remove the compact card instead of the status message.
// Greptile P1 on commit 0531913b.
const compactingStatusIds: Map<string, string> = new Map();

function shouldRenderCompactCard(p: CompactPayload): boolean {
	const key = p.sessionKey || "__active__";
	const now = Date.now();
	const previous = lastCompactCardAt.get(key) || 0;
	if (now - previous < COMPACT_CARD_DEBOUNCE_MS) {
		return false;
	}
	lastCompactCardAt.set(key, now);
	return true;
}

// Drop the "Compacting conversation..." status message the auto-compact
// start phase appended for this session, if one exists. Called by both
// compact-done handlers before rendering the card so the status message
// never outlives its purpose, regardless of which event arrives first.
function removeCompactingStatus(p: CompactPayload): void {
	const key = p.sessionKey || "__active__";
	const id = compactingStatusIds.get(key);
	compactingStatusIds.delete(key);
	if (id) removeItem(id);
}

function resetTokensAfterCompaction(): void {
	S.setSessionTokens({ input: 0, output: 0 });
	S.setSessionCurrentInputTokens(0);
	S.setSessionCurrentContextTokens(0);
	updateTokenBar();
}

function handleChatAutoCompact(p: ChatPayload, isActive: boolean, isChatPage: boolean): void {
	if (!(isActive && isChatPage)) return;
	if (p.phase === "start") {
		const statusId = addNotice("system", "Compacting conversation (context limit reached)\u2026");
		compactingStatusIds.set(p.sessionKey || "__active__", statusId);
	} else if (p.phase === "done") {
		// Always drop the status message -- even when the card was
		// already rendered by an earlier `chat.compact done` event.
		removeCompactingStatus(p as CompactPayload);
		if (shouldRenderCompactCard(p as CompactPayload)) {
			renderCompactCard(p);
		}
		resetTokensAfterCompaction();
	} else if (p.phase === "error") {
		removeCompactingStatus(p as CompactPayload);
		chatAddMsg("error", `Auto-compact failed: ${p.error?.message || p.error?.detail || "unknown error"}`);
	}
}

// `chat.compact done` is emitted by ChatService::compact on every
// compaction run (manual `/compact` RPCs AND the pre-emptive auto-
// compact path). It carries the mode/tokens/settings metadata from
// CompactionOutcome::broadcast_metadata() so the same card renders.
function handleChatCompact(p: ChatPayload, isActive: boolean, isChatPage: boolean): void {
	if (!(isActive && isChatPage)) return;
	if (p.phase !== "done") return;
	// Drop the auto-compact status message if one exists. For the
	// manual `/compact` RPC path there is no status message, so this
	// is a no-op. For `send()`'s pre-emptive auto-compact path,
	// `chat.compact done` arrives BEFORE `auto_compact done`, so we
	// clear the status message here; the subsequent `auto_compact done`
	// handler will find the slot already empty.
	removeCompactingStatus(p as CompactPayload);
	if (!shouldRenderCompactCard(p as CompactPayload)) return;
	renderCompactCard(p);
	resetTokensAfterCompaction();
}

// ── Retry handling ────────────────────────────────────────────

function retryDelayMsFromPayload(p: ChatPayload): number {
	if (p.retryAfterMs !== undefined && p.retryAfterMs !== null) return Number(p.retryAfterMs) || 0;
	if (p.error?.retryAfterMs !== undefined && p.error?.retryAfterMs !== null) return Number(p.error.retryAfterMs) || 0;
	return 0;
}

function retryStatusText(p: ChatPayload): string {
	const retryMs = retryDelayMsFromPayload(p);
	const retrySecs = Math.max(1, Math.ceil(retryMs / 1000));
	const rateLimited = p.error?.type === "rate_limit_exceeded";
	return rateLimited
		? `Rate limited by provider, retrying in ${retrySecs}s\u2026`
		: `Temporary provider issue, retrying in ${retrySecs}s\u2026`;
}

function handleChatRetrying(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	updateSessionRunId(eventSession, p.runId);
	setSessionReplying(eventSession, true);
	if (!(isActive && isChatPage)) return;
	setComposerStopButton(true, eventSession);
	clearChatEmptyState();
	showThinking(retryStatusText(p), "retry");
}

// ── Error / abort / notice / clear ────────────────────────────

function appendContinueButton(card: HTMLElement | null): void {
	const body = card?.querySelector(".error-body");
	if (!body) return;
	const btn = document.createElement("button");
	btn.className = "provider-btn error-continue-btn";
	btn.textContent = t("errors:chat.continue", "Continue");
	btn.onclick = () => {
		btn.disabled = true;
		btn.textContent = t("errors:chat.continuing", "Continuing...");
		(S.chatInput as HTMLInputElement).value = t("errors:chat.continueMessage", "Please continue where you left off.");
		S.chatSendBtn?.click();
	};
	body.appendChild(btn);
}

function handleChatError(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	setSessionReplying(eventSession, false);
	setSessionActiveRunId(eventSession, null);
	const errSession = sessionStore.getByKey(eventSession);
	if (errSession) errSession.resetStreamState();
	if (!(isActive && isChatPage)) {
		S.setVoicePending(false);
		return;
	}
	setComposerStopButton(false);
	hideThinking();
	closeStream();
	settleRunningTools("cancelled");
	const card = p.error?.title
		? chatAddErrorCard(localizeStructuredError(p.error) as Parameters<typeof chatAddErrorCard>[0])
		: chatAddErrorMsg(p.message || "unknown");
	// max_iterations_reached can be resumed.
	if (p.error?.canCancel && eventSession) addQuotaWaitCancelButton(eventSession);
	if (p.error?.canContinue) appendContinueButton(card);
	S.setStreamEl(null);
	S.setStreamText("");
	S.setVoicePending(false);
	moveFirstQueuedToChat();
}

function getAbortedPartialState(p: ChatPayload): AbortedPartialState {
	const partial = p.partialMessage && typeof p.partialMessage === "object" ? p.partialMessage : null;
	const partialText = String(partial?.content || "");
	const partialReasoning = String(partial?.reasoning || "");
	return {
		partial,
		partialText,
		partialReasoning,
		hasVisiblePartial: hasNonWhitespaceContent(partialText) || hasNonWhitespaceContent(partialReasoning),
	};
}

function cacheAbortedPartial(
	eventSession: string,
	p: ChatPayload,
	abortSession: ReturnType<typeof sessionStore.getByKey>,
	partialState: AbortedPartialState,
): void {
	if (!partialState.hasVisiblePartial) return;
	const partial = partialState.partial;
	const lastIdx = abortSession ? abortSession.lastHistoryIndex.value : S.lastHistoryIndex;
	if (p.messageIndex === undefined || p.messageIndex === null || p.messageIndex > lastIdx) {
		bumpSessionCount(eventSession, 1);
	}
	cacheSessionHistoryMessage(
		eventSession,
		{
			role: "assistant",
			content: partialState.partialText,
			model: partial?.model || "",
			provider: partial?.provider || "",
			inputTokens: partial?.inputTokens || 0,
			outputTokens: partial?.outputTokens || 0,
			durationMs: partial?.durationMs || 0,
			requestInputTokens: partial?.requestInputTokens,
			requestOutputTokens: partial?.requestOutputTokens,
			reasoning: partial?.reasoning || undefined,
			audio: partial?.audio || undefined,
			run_id: partial?.run_id || p.runId || undefined,
			created_at: partial?.created_at || Date.now(),
		},
		p.messageIndex,
	);
	updateSessionHistoryIndex(eventSession, p.messageIndex);
}

function renderAbortedPartial(p: ChatPayload, eventSession: string, partialState: AbortedPartialState): void {
	if (!partialState.hasVisiblePartial) {
		closeStream();
		return;
	}
	const partial = partialState.partial;
	const reasoning =
		partialState.partialReasoning && !isReasoningAlreadyShown(partialState.partialReasoning)
			? partialState.partialReasoning
			: "";
	const text = hasNonWhitespaceContent(partialState.partialText) ? partialState.partialText : streamedText();
	if (!(hasNonWhitespaceContent(text) || reasoning)) {
		closeStream();
		return;
	}
	finalizeAssistant({
		text,
		reasoning,
		usage: usageFromPayload({
			model: partial?.model,
			provider: partial?.provider,
			inputTokens: partial?.inputTokens,
			outputTokens: partial?.outputTokens,
			durationMs: partial?.durationMs,
		}),
		replyMedium: p.replyMedium || "text",
		runId: p.runId,
		historyIndex: p.messageIndex,
		createdAt: Date.now(),
		audioSrc: partial?.audio ? mediaSrc(eventSession, partial.audio) : undefined,
	});
}

function handleChatAborted(p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	setSessionReplying(eventSession, false);
	setSessionActiveRunId(eventSession, null);
	const partialState = getAbortedPartialState(p);
	const abortSession = sessionStore.getByKey(eventSession);
	cacheAbortedPartial(eventSession, p, abortSession, partialState);
	if (abortSession) abortSession.resetStreamState();
	if (partialState.hasVisiblePartial && !isActive) {
		setSessionUnread(eventSession, true);
	}
	if (!(isActive && isChatPage)) {
		S.setVoicePending(false);
		return;
	}
	setComposerStopButton(false);
	hideThinking();
	settleRunningTools("cancelled");
	renderAbortedPartial(p, eventSession, partialState);
	S.setStreamEl(null);
	S.setStreamText("");
	S.setVoicePending(false);
	moveFirstQueuedToChat();
}

function noticeClassName(p: ChatPayload): string {
	if (!p.title) return "";
	const classes = ["system-notice"];
	if (String(p.title).toLowerCase() === "sandbox") {
		classes.push("system-notice-sandbox");
		const message = String(p.message || "").toLowerCase();
		if (message.includes("enabled")) classes.push("is-enabled");
		else if (message.includes("disabled")) classes.push("is-disabled");
	}
	return classes.join(" ");
}

function handleChatNotice(p: ChatPayload, isActive: boolean, isChatPage: boolean): void {
	if (!(isActive && isChatPage)) return;
	if (!p.title) {
		addNotice("system", p.message || "");
		return;
	}
	// Titled notices are markdown so the title can be bold.
	addNotice("system", renderMarkdown(`**${p.title}:** ${p.message}`), {
		html: true,
		className: noticeClassName(p),
	});
}

function handleChatQueueCleared(_p: ChatPayload, _isActive: boolean, _isChatPage: boolean, eventSession: string): void {
	clearQueuedMessages(eventSession);
}

function handleChatQueueItemRemoved(
	p: ChatPayload,
	_isActive: boolean,
	_isChatPage: boolean,
	eventSession: string,
): void {
	if (typeof p.seq === "number") handleQueueItemRemoved(eventSession, p.seq);
}

function handleChatSessionCleared(_p: ChatPayload, isActive: boolean, isChatPage: boolean, eventSession: string): void {
	setSessionActiveRunId(eventSession, null);
	clearSessionHistoryCache(eventSession);
	// Reset badge, unread state, and history index for every client.
	markSessionLocallyCleared(eventSession);
	if (isActive) {
		S.setLastHistoryIndex(-1);
		S.setChatSeq(0);
	}
	if (!(isActive && isChatPage)) return;
	setComposerStopButton(false);
	clearTranscript();
	S.setSessionTokens({ input: 0, output: 0 });
	S.setSessionCurrentInputTokens(0);
	S.setSessionCurrentContextTokens(0);
	updateTokenBar();
}

// ── Quota waits ───────────────────────────────────────────────

function handleChatQuotaWaitCancelled(
	_p: ChatPayload,
	_isActive: boolean,
	_isChatPage: boolean,
	eventSession: string,
): void {
	const cards = S.chatMsgBox?.querySelectorAll<HTMLElement>(".error-card[data-quota-wait-session]") || [];
	for (const card of cards) {
		if (card.dataset.quotaWaitSession !== eventSession) continue;
		card.querySelector(".quota-wait-cancel-btn")?.remove();
		card.querySelector(".error-countdown")?.remove();
		const note = document.createElement("div");
		note.className = "error-detail quota-wait-cancelled";
		note.textContent = t("errors:chat.waitCancelled", "Cancelled: the run will not resume.");
		card.querySelector(".error-body")?.appendChild(note);
		delete card.dataset.quotaWaitSession;
	}
}

// ── Handler map and dispatcher ────────────────────────────────

export const chatHandlers: Record<string, ChatHandler> = {
	thinking: handleChatThinking,
	thinking_text: handleChatThinkingText,
	thinking_done: handleChatThinkingDone,
	voice_pending: handleChatVoicePending,
	tool_call_start: handleChatToolCallStart,
	tool_call_end: handleChatToolCallEnd,
	channel_user: handleChatChannelUser,
	user_message: handleChatUserMessage,
	delta: handleChatDelta,
	final: handleChatFinal,
	auto_compact: handleChatAutoCompact,
	compact: handleChatCompact,
	retrying: handleChatRetrying,
	error: handleChatError,
	quota_wait_cancelled: handleChatQuotaWaitCancelled,
	aborted: handleChatAborted,
	notice: handleChatNotice,
	queue_cleared: handleChatQueueCleared,
	queue_item_removed: handleChatQueueItemRemoved,
	session_cleared: handleChatSessionCleared,
};

export function handleChatEvent(p: ChatPayload): void {
	const eventSession = p.sessionKey || sessionStore.activeSessionKey.value;
	const isActive = eventSession === sessionStore.activeSessionKey.value;
	const isChatPage = currentPrefix === "/chats";

	if (isActive && sessionStore.switchInProgress.value) {
		// If session switching got stuck (e.g. lost RPC response), do not drop
		// terminal frames. Unstick and process final/error so replies still show
		// without requiring a full page reload.
		const allowDuringSwitch =
			p.state === "final" ||
			p.state === "error" ||
			p.state === "aborted" ||
			p.state === "notice" ||
			p.state === "session_cleared" ||
			p.state === "queue_cleared" ||
			p.state === "queue_item_removed";
		if (!allowDuringSwitch) {
			return;
		}
		if (p.state === "final" || p.state === "error" || p.state === "aborted") {
			sessionStore.switchInProgress.value = false;
			S.setSessionSwitchInProgress(false);
		}
	}

	if (p.sessionKey && !sessionStore.getByKey(p.sessionKey)) {
		fetchSessions();
	}

	const handler = chatHandlers[p.state || ""];
	if (handler) handler(p, isActive, isChatPage, eventSession);
}
