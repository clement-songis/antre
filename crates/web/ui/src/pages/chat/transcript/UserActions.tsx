// ── Edit and rewind actions on a user message ───────────────

import { useEffect, useRef, useState } from "preact/hooks";
import { sessionStore } from "../../../stores/session-store";
import { reportRewindError } from "../RewindDialog";
import { editAndResend, fetchRewindPoints, pointForUserItem, rewindTarget } from "../rewind";
import type { UserItem } from "./model";

interface UserActionsProps {
	item: UserItem;
	sessionKey: string;
}

async function resolvePoint(item: UserItem, sessionKey: string) {
	const points = await fetchRewindPoints(sessionKey);
	const point = pointForUserItem(item, points);
	if (!point) throw new Error("This message is not in the saved history yet");
	return { point, points };
}

export function UserActions({ item, sessionKey }: UserActionsProps) {
	const [editing, setEditing] = useState(false);
	const [hasText, setHasText] = useState(true);
	const [busy, setBusy] = useState(false);
	const textareaRef = useRef<HTMLTextAreaElement>(null);
	const replying = sessionStore.getByKey(sessionKey)?.replying.value === true;

	useEffect(() => {
		if (!editing) return;
		const el = textareaRef.current;
		if (!el) return;
		el.focus();
		el.setSelectionRange(el.value.length, el.value.length);
	}, [editing]);

	const openRewind = async () => {
		try {
			const { point, points } = await resolvePoint(item, sessionKey);
			rewindTarget.value = { sessionKey, point, points };
		} catch (err) {
			reportRewindError(err);
		}
	};

	const submitEdit = async () => {
		const text = textareaRef.current?.value.trim() || "";
		if (!text || busy) return;
		setBusy(true);
		try {
			const { point } = await resolvePoint(item, sessionKey);
			await editAndResend(sessionKey, point, text);
		} catch (err) {
			reportRewindError(err);
			setBusy(false);
		}
	};

	if (editing) {
		return (
			<div className="msg-user-edit flex flex-col gap-1.5 mt-2">
				<textarea
					ref={textareaRef}
					className="msg-user-edit-input w-full min-h-[64px] rounded-md border border-[var(--border)] bg-[var(--bg)] text-[var(--text)] p-2 text-sm font-[var(--font-body)]"
					aria-label="Edit message"
					defaultValue={item.text}
					disabled={busy}
					onInput={(e) => setHasText((e.target as HTMLTextAreaElement).value.trim().length > 0)}
					onKeyDown={(e) => {
						if (e.key === "Escape") {
							e.preventDefault();
							e.stopPropagation();
							setEditing(false);
						} else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
							e.preventDefault();
							void submitEdit();
						}
					}}
				/>
				<div className="flex gap-1.5 justify-end">
					<button
						type="button"
						className="provider-btn provider-btn-secondary provider-btn-sm"
						disabled={busy}
						onClick={() => setEditing(false)}
					>
						Cancel
					</button>
					<button
						type="button"
						className="provider-btn provider-btn-sm"
						disabled={busy || !hasText}
						title="Fork the conversation before this message and send the edited text there (Ctrl/Cmd+Enter)"
						onClick={() => void submitEdit()}
					>
						{busy ? "Sending…" : "Send as new branch"}
					</button>
				</div>
			</div>
		);
	}

	return (
		<div className="msg-user-actions">
			<button
				type="button"
				className="msg-action-btn"
				title="Edit and resend in a new branch"
				aria-label="Edit message"
				disabled={replying || item.images.length > 0 || item.documents.length > 0}
				onClick={() => {
					setHasText(item.text.trim().length > 0);
					setEditing(true);
				}}
			>
				<span className="icon icon-sm icon-code" aria-hidden="true" />
			</button>
			<button
				type="button"
				className="msg-action-btn"
				title="Rewind to before this message"
				aria-label="Rewind"
				disabled={replying}
				onClick={() => void openRewind()}
			>
				<span className="icon icon-sm icon-retry" aria-hidden="true" />
			</button>
		</div>
	);
}
