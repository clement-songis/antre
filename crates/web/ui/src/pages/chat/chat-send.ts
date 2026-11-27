// ── Chat send logic ──────────────────────────────────────────

import { chatAddMsg, scrollChatToBottom, setComposerStopButton } from "../../chat-ui";
import { renderMarkdown, sendRpc, warmAudioPlayback } from "../../helpers";
import {
	getPendingAttachments,
	hasPendingAttachments,
	type PendingAttachment,
	removePendingAttachments,
	type UploadedDocumentFile,
	uploadDocumentAttachment,
} from "../../media-drop";
import { setSessionModel } from "../../models";
import {
	addSessionSendError,
	bumpSessionCount,
	cacheOutgoingUserMessage,
	clearSessionSendErrors,
	seedSessionPreviewFromUserText,
	setSessionActiveRunId,
	setSessionReplying,
} from "../../sessions";
import * as S from "../../state";
import { modelStore } from "../../stores/model-store";
import type { RpcResponse } from "../../types/rpc";
import { handleSlashCommand, parseSlashCommand, shouldHandleSlashLocally, slashHideMenu } from "./slash-commands";
import { addNotice, addUserMessage, type UserMessageFields } from "./transcript/actions";
import type { UserItem } from "./transcript/model";
import { getItem, removeItem, removeItems } from "./transcript/store";

// ── Types ────────────────────────────────────────────────────

export interface ChatSendParams {
	text?: string;
	content?: ChatContentPart[];
	_document_files?: UploadedDocumentFile[];
	_session_key?: string;
	_seq: number;
	model?: string;
}

export type ChatContentPart = { type: "text"; text: string } | { type: "image_url"; image_url: { url: string } };

interface PendingImageAttachment extends PendingAttachment {
	dataUrl: string;
}

export interface ChatSendPayload {
	runId?: string;
	queued?: boolean;
}

function forActiveSession<T>(sessionKey: string, action: () => T): T | undefined {
	if (S.activeSessionKey === sessionKey) return action();
}

function handleChatSendFailure(sessionKey: string, message: string): void {
	setSessionReplying(sessionKey, false);
	addSessionSendError(sessionKey, message);
	forActiveSession(sessionKey, () => {
		setComposerStopButton(false, sessionKey);
		addNotice("error", message, { className: "chat-send-error" });
	});
}

// ── Auto-resize ─────────────────────────────────────────────

function chatAutoResize(): void {
	if (!S.chatInput) return;
	S.chatInput.style.height = "auto";
	S.chatInput.style.height = `${Math.min(S.chatInput.scrollHeight, 120)}px`;
}

// ── Slash command integration ───────────────────────────────

export function tryHandleLocalSlashCommand(text: string, hasAttachments: boolean): boolean {
	if (text.charAt(0) !== "/" || hasAttachments) return false;
	const slash = parseSlashCommand(text);
	if (!(slash && shouldHandleSlashLocally(slash.name, slash.args))) return false;
	(S.chatInput as HTMLTextAreaElement).value = "";
	chatAutoResize();
	slashHideMenu();
	handleSlashCommand(slash.name, slash.args);
	return true;
}

// ── History navigation ──────────────────────────────────────

export function handleHistoryUp(): void {
	if (S.chatHistory.length === 0) return;
	if (S.chatHistoryIdx === -1) {
		S.setChatHistoryDraft((S.chatInput as HTMLTextAreaElement).value);
		S.setChatHistoryIdx(S.chatHistory.length - 1);
	} else if (S.chatHistoryIdx > 0) S.setChatHistoryIdx(S.chatHistoryIdx - 1);
	(S.chatInput as HTMLTextAreaElement).value = S.chatHistory[S.chatHistoryIdx];
	chatAutoResize();
}

export function handleHistoryDown(): void {
	if (S.chatHistoryIdx === -1) return;
	if (S.chatHistoryIdx < S.chatHistory.length - 1) {
		S.setChatHistoryIdx(S.chatHistoryIdx + 1);
		(S.chatInput as HTMLTextAreaElement).value = S.chatHistory[S.chatHistoryIdx];
	} else {
		S.setChatHistoryIdx(-1);
		(S.chatInput as HTMLTextAreaElement).value = S.chatHistoryDraft;
	}
	chatAutoResize();
}

// ── Send helpers ────────────────────────────────────────────

export function rememberChatHistory(text: string): void {
	if (!text) return;
	S.chatHistory.push(text);
	if (S.chatHistory.length > 200) S.setChatHistory(S.chatHistory.slice(-200));
	localStorage.setItem("moltis-chat-history", JSON.stringify(S.chatHistory));
}

export function resetComposerAfterSend(): void {
	S.setChatHistoryIdx(-1);
	S.setChatHistoryDraft("");
	(S.chatInput as HTMLTextAreaElement).value = "";
	chatAutoResize();
	if (window.innerWidth < 768) S.chatInput?.blur();
}

export function normalizeOutgoingText(text: string, hasAttachments: boolean): string {
	if (!(S.commandModeEnabled && text && !hasAttachments)) return text;
	const parsed = parseSlashCommand(text);
	if (parsed && parsed.name === "sh") return text;
	return `/sh ${text}`;
}

export function applySelectedModelToChatParams(chatParams: ChatSendParams, sessionKey: string): void {
	const effectiveId = modelStore.effectiveModelId.value;
	if (!effectiveId) return;
	chatParams.model = effectiveId;
	setSessionModel(sessionKey, effectiveId);
}

export function handleChatSendRpcResponse(
	res: RpcResponse<ChatSendPayload>,
	userItemId: string | null,
	sessionKey: string,
): void {
	if (res.ok && res.payload?.runId) setSessionActiveRunId(sessionKey, res.payload.runId);
	if (res.ok) {
		clearSessionSendErrors(sessionKey);
		forActiveSession(sessionKey, () => {
			removeItems((item) => item.kind === "notice" && item.className === "chat-send-error");
		});
	}
	if (res.payload?.queued) {
		forActiveSession(sessionKey, () => markMessageQueued(userItemId, sessionKey));
		return;
	}
	if (!res.ok && res.error) {
		handleChatSendFailure(sessionKey, res.error.message || "Request failed");
	}
}

/** Upload pending attachments and fold them into the send params. */
async function attachPendingFiles(
	params: ChatSendParams,
	text: string,
	sessionKey: string,
): Promise<{ images: PendingImageAttachment[]; documents: UploadedDocumentFile[] }> {
	const attachments = hasPendingAttachments() ? [...getPendingAttachments()] : [];
	if (attachments.length === 0) return { images: [], documents: [] };
	const images = attachments.filter((attachment): attachment is PendingImageAttachment => Boolean(attachment.dataUrl));
	const pendingDocuments = attachments.filter((attachment) => !attachment.dataUrl);
	const documents = await Promise.all(
		pendingDocuments.map((attachment) => uploadDocumentAttachment(attachment, sessionKey)),
	);
	const content: ChatContentPart[] = [];
	if (text) content.push({ type: "text", text });
	for (const img of images) content.push({ type: "image_url", image_url: { url: img.dataUrl } });
	if (content.length > 0) {
		params.content = content;
		delete params.text;
	}
	if (documents.length > 0) params._document_files = documents;
	removePendingAttachments(attachments);
	return { images, documents };
}

function queuedTrayElement(item: UserItem | undefined): HTMLElement {
	const el = document.createElement("div");
	el.className = "msg user";
	if (item?.text) {
		// Safe: renderMarkdown escapes its input before adding tags.
		el.innerHTML = renderMarkdown(item.text); // eslint-disable-line no-unsanitized/property
	}
	return el;
}

function markMessageQueued(userItemId: string | null, sessionKey: string): void {
	const tray = document.getElementById("queuedMessages");
	if (!(tray && userItemId)) return;
	const item = getItem(userItemId, "user");
	removeItem(userItemId);
	const el = queuedTrayElement(item);
	el.classList.add("queued");
	const badge = document.createElement("div");
	badge.className = "queued-badge";
	const label = document.createElement("span");
	label.className = "queued-label";
	label.textContent = "Queued";
	const btn = document.createElement("button");
	btn.className = "queued-cancel";
	btn.title = "Cancel all queued";
	btn.textContent = "\u2715";
	btn.addEventListener("click", (e: MouseEvent) => {
		e.stopPropagation();
		sendRpc("chat.cancel_queued", { sessionKey });
	});
	badge.appendChild(label);
	badge.appendChild(btn);
	el.appendChild(badge);
	tray.appendChild(el);
	tray.classList.remove("hidden");
}

export interface OutgoingMessage {
	sessionKey: string;
	/** Text sent to the model. */
	text: string;
	/** Text shown in the transcript when it differs (e.g. `/sh` wrapping). */
	displayText?: string;
	/** Fold the composer's pending attachments into this message. */
	withAttachments?: boolean;
	/** Extra `chat.send` params (`_input_medium`, `_audio_filename`, ...). */
	extraParams?: Record<string, unknown>;
	/** How the user message is shown; `false` sends without showing one. */
	display?: Partial<UserMessageFields> | false;
	/** Runs once attachments uploaded, before `chat.send` (clear the composer here). */
	onPrepared?: () => void;
}

/**
 * The one path every outgoing chat message takes: composer, voice input,
 * edit-and-resend. It renders the user message, keeps the session
 * bookkeeping in step and sends `chat.send`.
 */
export async function dispatchChatMessage(msg: OutgoingMessage): Promise<RpcResponse<ChatSendPayload>> {
	const { sessionKey } = msg;
	S.setChatSeq(S.chatSeq + 1);
	const params: ChatSendParams = { text: msg.text, _seq: S.chatSeq, ...(msg.extraParams || {}) };
	params._session_key = sessionKey;
	const shown = msg.displayText === undefined ? msg.text : msg.displayText;
	const files = msg.withAttachments ? await attachPendingFiles(params, msg.text, sessionKey) : null;
	msg.onPrepared?.();
	let userItemId: string | null = null;
	if (msg.display !== false) {
		userItemId =
			forActiveSession(sessionKey, () => {
				const id = addUserMessage({
					text: shown,
					images: files?.images.map((img) => ({ dataUrl: img.dataUrl, name: img.name || "image" })),
					documents: files?.documents,
					...(msg.display || {}),
				});
				scrollChatToBottom(true);
				return id;
			}) ?? null;
	}
	applySelectedModelToChatParams(params, sessionKey);
	bumpSessionCount(sessionKey, 1);
	cacheOutgoingUserMessage(sessionKey, params);
	seedSessionPreviewFromUserText(sessionKey, shown || msg.text);
	setSessionReplying(sessionKey, true);
	forActiveSession(sessionKey, () => setComposerStopButton(true, sessionKey));
	let res: RpcResponse<ChatSendPayload>;
	try {
		res = await sendRpc<ChatSendPayload>("chat.send", params);
	} catch {
		res = { ok: false, error: { code: "UNAVAILABLE", message: "Request failed" } };
	}
	handleChatSendRpcResponse(res, userItemId, sessionKey);
	forActiveSession(sessionKey, () => maybeRefreshFullContextFn?.());
	return res;
}

// ── Main sendChat function ──────────────────────────────────
// Exposed so ChatPage and slash-commands can call it.

let maybeRefreshFullContextFn: (() => void) | null = null;

/** Called by ChatPage to register the refresh callback. */
export function setMaybeRefreshFullContextFn(fn: () => void): void {
	maybeRefreshFullContextFn = fn;
}

let sendInProgress = false;

export function sendChat(): void {
	void sendChatAsync();
}

async function sendChatAsync(): Promise<void> {
	if (sendInProgress) return;
	const text = (S.chatInput as HTMLTextAreaElement).value.trim();
	const hasAttachments = hasPendingAttachments();
	if (!((text || hasAttachments) && S.connected)) return;
	const sessionKey = S.activeSessionKey;
	sendInProgress = true;
	warmAudioPlayback();
	try {
		if (tryHandleLocalSlashCommand(text, hasAttachments)) return;
		const outgoingText = normalizeOutgoingText(text, hasAttachments);
		await dispatchChatMessage({
			sessionKey,
			text: outgoingText,
			displayText: text,
			withAttachments: hasAttachments,
			// After a successful upload only: a failed one keeps the typed text.
			onPrepared: () => {
				rememberChatHistory(text);
				forActiveSession(sessionKey, resetComposerAfterSend);
			},
		});
	} catch (err) {
		forActiveSession(sessionKey, () => {
			chatAddMsg("error", err instanceof Error ? err.message : "File upload failed");
		});
	} finally {
		sendInProgress = false;
	}
}

export { chatAutoResize };
