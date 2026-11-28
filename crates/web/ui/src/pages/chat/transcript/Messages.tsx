// ── Transcript message components ───────────────────────────
//
// Class names match the markup the imperative renderer produced (`.msg`,
// `.msg-model-footer`, `.msg-action-bar`, ...) so existing CSS applies.
// Leaf widgets that still build their own DOM (audio player, document card,
// action bar) are mounted into dedicated slots so Preact never diffs them.

import type { ComponentChildren } from "preact";
import { useEffect, useLayoutEffect, useRef } from "preact/hooks";
import { appendChannelFooter } from "../../../chat-ui";
import { contextMenuTrigger, MenuButton } from "../../../components/ContextMenu";
import {
	formatAssistantTokenUsage,
	formatTokenSpeed,
	renderAudioPlayer,
	renderDocument,
	tokenSpeedTone,
} from "../../../helpers";
import { t } from "../../../i18n";
import { appendMessageActions } from "../../../message-actions";
import { upsertTtsProviderFooter } from "../../../message-voice";
import { showThinkingTail } from "../../../ws/shared";
import { Markdown } from "./Markdown";
import { assistantMessageMenu, userMessageMenu } from "./message-menu";
import type { AssistantItem, AssistantUsage, DomItem, NoticeItem, ThinkingItem, UserItem } from "./model";
import { UserActions } from "./UserActions";

/** Runs `mount` once on a slot element; the slot content is owned by `mount`. */
function ImperativeSlot({ mount, deps }: { mount: (el: HTMLElement) => void; deps: unknown[] }) {
	const ref = useRef<HTMLDivElement>(null);
	useLayoutEffect(() => {
		const el = ref.current;
		if (!el) return;
		el.textContent = "";
		mount(el);
		// biome-ignore lint/correctness/useExhaustiveDependencies: callers pass the inputs `mount` reads.
	}, deps);
	return <div ref={ref} className="contents" />;
}

function documentMediaSrc(sessionKey: string, storedName: string, url?: string): string {
	if (url) return url;
	if (!storedName) return "#";
	return `/api/sessions/${encodeURIComponent(sessionKey)}/media/${encodeURIComponent(storedName)}`;
}

interface UserMessageProps {
	item: UserItem;
	sessionKey: string;
}

export function UserMessage({ item, sessionKey }: UserMessageProps) {
	const hasText = item.text.trim().length > 0;
	return (
		<div
			className="msg user"
			data-item-id={item.id}
			data-history-index={item.historyIndex}
			{...contextMenuTrigger(() => userMessageMenu(item, sessionKey))}
		>
			{item.audioSrc ? (
				<ImperativeSlot mount={(el) => renderAudioPlayer(el, item.audioSrc as string)} deps={[item.audioSrc]} />
			) : null}
			{hasText ? (
				<div className={item.audioSrc ? "mt-2" : "contents"}>
					<Markdown text={item.text} />
				</div>
			) : null}
			{item.images.length > 0 ? (
				<div className="msg-image-row">
					{item.images.map((img) => (
						<img key={img.dataUrl} className="msg-image-thumb" src={img.dataUrl} alt={img.name} />
					))}
				</div>
			) : null}
			{item.documents.length > 0 ? (
				<ImperativeSlot
					mount={(el) => {
						for (const doc of item.documents) {
							const src = documentMediaSrc(sessionKey, doc.stored_filename, doc.url);
							renderDocument(el, src, doc.display_name || doc.stored_filename, doc.mime_type, doc.size_bytes);
						}
					}}
					deps={[item.documents, sessionKey]}
				/>
			) : null}
			{item.channel ? (
				<ImperativeSlot
					mount={(el) => appendChannelFooter(el, item.channel as NonNullable<UserItem["channel"]>)}
					deps={[item.channel]}
				/>
			) : null}
			{item.footnote ? <div className="msg-model-footer">{item.footnote}</div> : null}
			<UserActions item={item} sessionKey={sessionKey} />
		</div>
	);
}

function ReasoningDisclosure({ text }: { text: string }) {
	const normalized = text.trim();
	if (!normalized) return null;
	return (
		<details className="msg-reasoning">
			<summary className="msg-reasoning-summary">{t("chat:a11y.reasoning")}</summary>
			<div className="msg-reasoning-body">{normalized}</div>
		</details>
	);
}

export { ReasoningDisclosure };

interface UsageFooterProps {
	usage?: AssistantUsage;
	replyMedium?: string;
	createdAt?: number;
	showTime: boolean;
}

export function UsageFooter({ usage, replyMedium, createdAt, showTime }: UsageFooterProps) {
	if (!(usage?.model || (showTime && createdAt))) return null;
	let label = "";
	if (usage?.model) {
		label = usage.provider ? `${usage.provider} / ${usage.model}` : usage.model;
		if (usage.inputTokens || usage.outputTokens) {
			label += ` \u00b7 ${formatAssistantTokenUsage(usage.inputTokens, usage.outputTokens, usage.cacheReadTokens)}`;
		}
	}
	const speed = usage ? formatTokenSpeed(usage.outputTokens, usage.durationMs) : null;
	const tone = usage ? tokenSpeedTone(usage.outputTokens, usage.durationMs) : null;
	return (
		<div className="msg-model-footer">
			{label ? <span>{label}</span> : null}
			{speed ? (
				<span className={`msg-token-speed${tone ? ` msg-token-speed-${tone}` : ""}`}>{` \u00b7 ${speed}`}</span>
			) : null}
			{replyMedium === "voice" || replyMedium === "text" ? (
				<span className="reply-medium-badge">{replyMedium}</span>
			) : null}
			{showTime && createdAt ? (
				<span className="msg-footer-time">
					{" \u00b7 "}
					<time className="msg-footer-time" data-epoch-ms={String(createdAt)}>
						{new Date(createdAt).toISOString()}
					</time>
				</span>
			) : null}
		</div>
	);
}

interface AssistantMessageProps {
	item: AssistantItem;
	sessionKey: string;
	isLast: boolean;
	/** Extra controls rendered after the action bar (rewind, regenerate). */
	children?: ComponentChildren;
}

export function AssistantMessage({ item, sessionKey, isLast, children }: AssistantMessageProps) {
	const hasText = item.text.trim().length > 0;
	const ref = useRef<HTMLDivElement>(null);
	useEffect(() => {
		if (ref.current && item.ttsProvider) upsertTtsProviderFooter(ref.current, item.ttsProvider);
	}, [item.ttsProvider]);
	// The action bar is built imperatively and appended last, as a direct
	// child, after every rendered part of the message.
	useLayoutEffect(() => {
		const host = ref.current;
		if (!(host && item.final)) return;
		for (const bar of host.querySelectorAll(":scope > .msg-action-bar")) bar.remove();
		appendMessageActions({
			messageEl: host,
			sessionKey,
			messageIndex: item.historyIndex,
			text: item.text,
			runId: item.runId,
			hasAudio: !!item.audioSrc,
			audioWarning: item.audioWarning,
		});
	}, [item.final, item.historyIndex, item.runId, item.text, sessionKey, item.audioSrc, item.audioWarning]);
	return (
		<div
			ref={ref}
			className={`msg assistant${item.streaming ? " streaming" : ""}`}
			data-item-id={item.id}
			data-history-index={item.historyIndex}
			data-run-id={item.runId}
			{...(item.final ? contextMenuTrigger(() => assistantMessageMenu(item, sessionKey)) : {})}
		>
			{item.audioSrc ? (
				<ImperativeSlot
					mount={(el) => renderAudioPlayer(el, item.audioSrc as string, item.audioAutoplay === true)}
					deps={[item.audioSrc]}
				/>
			) : null}
			{hasText ? (
				<div className={item.audioSrc ? "mt-2" : "contents"}>
					<Markdown text={item.text} streaming={item.streaming} />
				</div>
			) : null}
			{item.final ? <ReasoningDisclosure text={item.reasoning} /> : null}
			{item.final ? (
				<UsageFooter usage={item.usage} replyMedium={item.replyMedium} createdAt={item.createdAt} showTime={isLast} />
			) : null}
			{item.final ? (
				<div className="msg-menu-slot">
					<MenuButton label={t("chat:a11y.moreActions")} getMenu={() => assistantMessageMenu(item, sessionKey)} />
				</div>
			) : null}
			{children}
		</div>
	);
}

export function NoticeMessage({ item }: { item: NoticeItem }) {
	const className = `msg ${item.level}${item.level === "system" ? " system-notice" : ""}${item.className ? ` ${item.className}` : ""}`;
	if (item.html) {
		return (
			<div
				className={className}
				data-item-id={item.id}
				role={item.level === "error" ? "alert" : undefined}
				// biome-ignore lint/security/noDangerouslySetInnerHtml: html notices come from renderMarkdown, which escapes first.
				dangerouslySetInnerHTML={{ __html: item.text }}
			/>
		);
	}
	return (
		<div className={className} data-item-id={item.id} role={item.level === "error" ? "alert" : undefined}>
			{item.text}
		</div>
	);
}

export function ThinkingIndicator({ item }: { item: ThinkingItem }) {
	const textRef = useRef<HTMLSpanElement>(null);
	useLayoutEffect(() => {
		if (textRef.current) showThinkingTail(textRef.current);
	}, [item.text]);
	const loading = item.tone === "loading";
	return (
		<div
			className={`msg assistant thinking${loading ? " session-loading" : ""}`}
			id={loading ? "sessionLoadIndicator" : "thinkingIndicator"}
		>
			{item.text && !loading ? (
				<span ref={textRef} className="thinking-text">
					{item.text}
				</span>
			) : (
				<span className="thinking-dots">
					<span />
					<span />
					<span />
				</span>
			)}
			{loading ? <span className="session-loading-label">{item.text || t("chat:a11y.loadingSession")}</span> : null}
		</div>
	);
}

export function DomSlot({ item }: { item: DomItem }) {
	const ref = useRef<HTMLDivElement>(null);
	useLayoutEffect(() => {
		const host = ref.current;
		if (!host) return;
		if (item.node.parentNode !== host) host.appendChild(item.node);
	}, [item.node]);
	return <div ref={ref} className="contents" data-item-id={item.id} />;
}
