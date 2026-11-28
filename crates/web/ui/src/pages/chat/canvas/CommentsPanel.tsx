// ── Comment pins and frame errors under the preview ─────────

import { useEffect, useRef, useState } from "preact/hooks";
import { useTranslation } from "../../../i18n";
import { activeSessionKey } from "../../../stores/session-store";
import { targetValue } from "../../../typed-events";
import { dispatchChatMessage } from "../chat-send";
import { commentsPrompt, elementLabel } from "./prompts";
import {
	activeDesign,
	clearFrameErrors,
	comments,
	frameErrors,
	pendingComment,
	removeComment,
	resolveSentComments,
	saveComment,
	shownVersion,
} from "./store";
import type { PickedElement } from "./types";

interface CommentsPanelProps {
	/** Comment the frame asked to show (`pin.open`). */
	focusedId: string | null;
	onHighlight: (target: { mid?: string; selector?: string }) => void;
}

/** Keyed by the picked element, so each pick starts with an empty form. */
function PendingCommentForm({ element }: { element: PickedElement }) {
	const { t } = useTranslation("canvas");
	const [body, setBody] = useState("");
	const [saving, setSaving] = useState(false);
	const inputRef = useRef<HTMLTextAreaElement>(null);
	useEffect(() => {
		inputRef.current?.focus();
	}, []);
	const submit = async () => {
		setSaving(true);
		const saved = await saveComment(body);
		setSaving(false);
		if (saved) setBody("");
	};
	return (
		<form
			className="flex flex-col gap-2 rounded-corner-m border border-line bg-raised p-2"
			onSubmit={(event) => {
				event.preventDefault();
				void submit();
			}}
		>
			<span className="an-hint truncate" title={element.text}>
				{t("commentOn", { element: elementLabel(element) })}
			</span>
			<textarea
				ref={inputRef}
				className="an-input min-h-[60px] text-sm"
				aria-label={t("commentLabel")}
				placeholder={t("commentPlaceholder")}
				value={body}
				maxLength={2000}
				onInput={(event) => setBody(targetValue(event))}
				onKeyDown={(event) => {
					if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
						event.preventDefault();
						void submit();
					} else if (event.key === "Escape") {
						pendingComment.value = null;
					}
				}}
			/>
			<div className="flex justify-end gap-2">
				<button type="button" className="an-btn an-btn--ghost an-btn--sm" onClick={() => (pendingComment.value = null)}>
					{t("cancel")}
				</button>
				<button type="submit" className="an-btn an-btn--primary an-btn--sm" disabled={saving || !body.trim()}>
					{t("addComment")}
				</button>
			</div>
		</form>
	);
}

function FrameErrors() {
	const { t } = useTranslation("canvas");
	const errors = frameErrors.value;
	const design = activeDesign.value;
	if (!(errors.length && design)) return null;
	const sendErrors = () => {
		const lines = errors.map((error) => {
			const where = error.source ? ` (${error.source}${error.line ? `:${error.line}` : ""})` : "";
			return `- ${error.message}${where}`;
		});
		void dispatchChatMessage({
			sessionKey: activeSessionKey.value,
			text: [`The preview of design "${design.title}" (design_id: ${design.id}) reports errors:`, ...lines].join("\n"),
		});
		clearFrameErrors();
	};
	return (
		<div className="an-banner an-banner--warn flex-col items-stretch gap-1" role="status">
			<div className="flex items-center gap-2">
				<span className="an-i an-i-warn an-i--sm" aria-hidden="true" />
				<span className="an-strong flex-1">{t("frameErrors", { count: errors.length })}</span>
				<button type="button" className="an-btn an-btn--sm" onClick={sendErrors}>
					{t("sendErrors")}
				</button>
				<button type="button" className="an-btn an-btn--ghost an-btn--sm" onClick={clearFrameErrors}>
					{t("dismiss")}
				</button>
			</div>
			<ul className="m-0 max-h-24 list-none overflow-y-auto p-0 font-code text-xs">
				{errors.map((error, index) => (
					// biome-ignore lint/suspicious/noArrayIndexKey: errors are append-only and positional.
					<li key={index} className="truncate" title={error.message}>
						{error.message}
					</li>
				))}
			</ul>
		</div>
	);
}

export function CommentsPanel({ focusedId, onHighlight }: CommentsPanelProps) {
	const { t } = useTranslation("canvas");
	const list = comments.value;
	const design = activeDesign.value;
	const listRef = useRef<HTMLOListElement>(null);
	useEffect(() => {
		if (!focusedId) return;
		listRef.current?.querySelector(`[data-comment-id="${CSS.escape(focusedId)}"]`)?.scrollIntoView({
			block: "nearest",
		});
	}, [focusedId]);
	const version = shownVersion.value ?? design?.currentVersion ?? 0;
	const sendToAgent = async () => {
		if (!(design && list.length)) return;
		const sent = [...list];
		const res = await dispatchChatMessage({
			sessionKey: activeSessionKey.value,
			text: commentsPrompt(design, version, sent),
		});
		if (res.ok) await resolveSentComments(sent);
	};
	return (
		<div className="flex flex-col gap-2" data-testid="canvas-comments">
			<FrameErrors />
			{pendingComment.value ? (
				<PendingCommentForm
					key={`${pendingComment.value.selector}@${pendingComment.value.rect.x},${pendingComment.value.rect.y}`}
					element={pendingComment.value}
				/>
			) : null}
			{list.length > 0 && (
				<div className="flex items-center gap-2">
					<span className="an-label flex-1">{t("comments", { count: list.length })}</span>
					<button type="button" className="an-btn an-btn--primary an-btn--sm" onClick={() => void sendToAgent()}>
						<span className="an-i an-i-send an-i--sm" aria-hidden="true" />
						{t("sendToAgent")}
					</button>
				</div>
			)}
			{list.length > 0 && (
				<ol ref={listRef} className="m-0 flex max-h-48 list-none flex-col gap-1 overflow-y-auto p-0">
					{list.map((comment, index) => (
						<li
							key={comment.id}
							data-comment-id={comment.id}
							className={`flex items-start gap-2 rounded-corner-s border p-2 text-sm ${
								comment.id === focusedId ? "border-lake bg-lake-wash" : "border-line bg-raised"
							}`}
						>
							<button
								type="button"
								className="an-avatar h-6 w-6 shrink-0 cursor-pointer border-0 bg-lake text-[11px] text-on-lake"
								aria-label={t("showComment", { n: index + 1 })}
								onClick={() => onHighlight({ mid: comment.mid, selector: comment.selector })}
							>
								{index + 1}
							</button>
							<div className="min-w-0 flex-1">
								<div className="whitespace-pre-wrap break-words text-ink-strong">{comment.body}</div>
								<div className="an-hint truncate">
									{comment.mid || comment.selector
										? elementLabel({ mid: comment.mid, selector: comment.selector ?? "" })
										: ""}
								</div>
							</div>
							<button
								type="button"
								className="an-btn an-btn--ghost an-btn--sm an-btn--icon"
								aria-label={t("deleteComment")}
								title={t("deleteComment")}
								onClick={() => void removeComment(comment.id)}
							>
								<span className="an-i an-i-trash an-i--sm" aria-hidden="true" />
							</button>
						</li>
					))}
				</ol>
			)}
		</div>
	);
}
