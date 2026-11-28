// ── Edit and rewind actions on a user message ───────────────

import { signal } from "@preact/signals";
import { useLayoutEffect, useRef, useState } from "preact/hooks";
import { MenuButton } from "../../../components/ContextMenu";
import { t, useTranslation } from "../../../i18n";
import { sessionStore } from "../../../stores/session-store";
import { reportRewindError } from "../RewindDialog";
import { editAndResend, fetchRewindPoints, pointForUserItem, rewindTarget } from "../rewind";
import { userMessageMenu } from "./message-menu";
import type { UserItem } from "./model";

interface UserActionsProps {
	item: UserItem;
	sessionKey: string;
}

/** The user message being edited in place, if any. */
export const editingUserItemId = signal<string | null>(null);

export async function resolvePoint(item: UserItem, sessionKey: string) {
	const points = await fetchRewindPoints(sessionKey);
	const point = pointForUserItem(item, points);
	if (!point) throw new Error(t("chat:rewind.notSaved"));
	return { point, points };
}

/** Open the rewind dialog for this user message. */
export async function openRewindFor(item: UserItem, sessionKey: string): Promise<void> {
	try {
		const { point, points } = await resolvePoint(item, sessionKey);
		rewindTarget.value = { sessionKey, point, points };
	} catch (err) {
		reportRewindError(err);
	}
}

export function UserActions({ item, sessionKey }: UserActionsProps) {
	const { t: chatT } = useTranslation("chat");
	const editing = editingUserItemId.value === item.id;
	const setEditing = (value: boolean) => {
		editingUserItemId.value = value ? item.id : null;
	};
	const [hasText, setHasText] = useState(true);
	const [busy, setBusy] = useState(false);
	const textareaRef = useRef<HTMLTextAreaElement>(null);
	const replying = sessionStore.getByKey(sessionKey)?.replying.value === true;

	useLayoutEffect(() => {
		if (!editing) return;
		const el = textareaRef.current;
		if (!el) return;
		// Set before paint so nothing typed meanwhile is overwritten.
		el.value = item.text;
		el.focus();
		el.setSelectionRange(el.value.length, el.value.length);
	}, [editing]);

	const openRewind = () => openRewindFor(item, sessionKey);

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
					className="msg-user-edit-input w-full min-h-[64px] rounded-md border border-line bg-ground text-ink p-2 text-sm font-text"
					aria-label={chatT("editMessage.label")}
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
						{chatT("editMessage.cancel")}
					</button>
					<button
						type="button"
						className="provider-btn provider-btn-sm"
						disabled={busy || !hasText}
						title={chatT("editMessage.sendTitle")}
						onClick={() => void submitEdit()}
					>
						{busy ? chatT("editMessage.sending") : chatT("editMessage.send")}
					</button>
				</div>
			</div>
		);
	}

	return (
		<div className="msg-user-actions">
			<MenuButton label={chatT("a11y.moreActions")} getMenu={() => userMessageMenu(item, sessionKey)} />
			<button
				type="button"
				className="msg-action-btn"
				title={chatT("editMessage.buttonTitle")}
				aria-label={chatT("editMessage.label")}
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
				title={chatT("rewind.rewindTitle")}
				aria-label={chatT("rewind.title")}
				disabled={replying}
				onClick={() => void openRewind()}
			>
				<span className="icon icon-sm icon-retry" aria-hidden="true" />
			</button>
		</div>
	);
}
