// ── Rewind dialog ───────────────────────────────────────────
//
// Opened from a user message's "Rewind" action (one turn) or with Esc Esc
// (pick a turn first). Offers restoring the conversation, the files the
// later turns changed, or both.

import { useEffect, useRef, useState } from "preact/hooks";
import { t, useTranslation } from "../../i18n";
import { showToast } from "../../ui";
import { type RewindPoint, type RewindScope, rewindPickerOpen, rewindTarget, rewindTo, runIdsFrom } from "./rewind";

function preview(text: string): string {
	const line = text.trim().split("\n")[0] || t("chat:rewind.attachment");
	return line.length > 120 ? `${line.slice(0, 120)}…` : line;
}

interface ScopeChoiceProps {
	sessionKey: string;
	point: RewindPoint;
	points: RewindPoint[];
	onDone: () => void;
}

function ScopeChoice({ sessionKey, point, points, onDone }: ScopeChoiceProps) {
	const { t: chatT } = useTranslation("chat");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const hasFileChanges = runIdsFrom(point, points).length > 0;
	const firstRef = useRef<HTMLButtonElement>(null);
	useEffect(() => firstRef.current?.focus(), [point.index]);
	const run = async (scope: RewindScope) => {
		setBusy(true);
		setError(null);
		try {
			await rewindTo(sessionKey, point, points, scope);
			onDone();
		} catch (err) {
			setError(err instanceof Error ? err.message : chatT("rewind.failed"));
			setBusy(false);
		}
	};
	return (
		<div className="flex flex-col gap-2">
			<div className="text-xs text-ink-muted">{chatT("rewind.before")}</div>
			<blockquote className="rewind-quote m-0 text-sm border-l-2 border-line pl-2">{preview(point.text)}</blockquote>
			<div className="flex flex-col gap-1.5">
				<button ref={firstRef} type="button" className="provider-btn" disabled={busy} onClick={() => void run("both")}>
					{chatT("rewind.both")}
				</button>
				<button
					type="button"
					className="provider-btn provider-btn-secondary"
					disabled={busy}
					onClick={() => void run("conversation")}
				>
					{chatT("rewind.conversation")}
				</button>
				<button
					type="button"
					className="provider-btn provider-btn-secondary"
					disabled={busy || !hasFileChanges}
					title={hasFileChanges ? chatT("rewind.filesTitle") : chatT("rewind.noFiles")}
					onClick={() => void run("files")}
				>
					{chatT("rewind.files")}
				</button>
			</div>
			<div className="text-xs text-ink-muted">{chatT("rewind.explain")}</div>
			{error ? (
				<div className="text-xs text-state-error" role="alert">
					{error}
				</div>
			) : null}
		</div>
	);
}

export function RewindDialog() {
	const { t: chatT } = useTranslation("chat");
	const target = rewindTarget.value;
	const picker = rewindPickerOpen.value;
	const [picked, setPicked] = useState<RewindPoint | null>(null);
	const open = !!(target || picker);
	const close = () => {
		rewindTarget.value = null;
		rewindPickerOpen.value = null;
		setPicked(null);
	};
	useEffect(() => {
		if (!open) return;
		const onKey = (e: KeyboardEvent) => {
			if (e.key !== "Escape") return;
			e.preventDefault();
			e.stopPropagation();
			close();
		};
		document.addEventListener("keydown", onKey, true);
		return () => document.removeEventListener("keydown", onKey, true);
	}, [open]);
	if (!open) return null;
	const sessionKey = (target || picker)?.sessionKey as string;
	const points = (target?.points || picker?.points || []) as RewindPoint[];
	const point = target?.point || picked;
	return (
		<div
			className="provider-modal-backdrop"
			onClick={(e) => {
				if (e.target === e.currentTarget) close();
			}}
		>
			<div
				className="provider-modal rewind-dialog w-[min(520px,94vw)]"
				role="dialog"
				aria-modal="true"
				aria-label={chatT("rewind.title")}
			>
				<div className="provider-modal-header">
					<div className="provider-item-name">{chatT("rewind.title")}</div>
					<button type="button" className="provider-btn provider-btn-secondary provider-btn-sm" onClick={close}>
						{chatT("rewind.close")}
					</button>
				</div>
				<div className="provider-modal-body">
					{point ? (
						<ScopeChoice
							sessionKey={sessionKey}
							point={point}
							points={points}
							onDone={() => {
								close();
							}}
						/>
					) : points.length === 0 ? (
						<div className="text-sm text-ink-muted">{chatT("rewind.empty")}</div>
					) : (
						<ul className="rewind-turns m-0 p-0 list-none flex flex-col gap-1 max-h-[50vh] overflow-y-auto">
							{[...points].reverse().map((entry) => (
								<li key={entry.index}>
									<button
										type="button"
										className="rewind-turn w-full text-left text-sm px-2 py-1.5 rounded-md border border-line bg-transparent cursor-pointer hover:bg-raised-2"
										onClick={() => setPicked(entry)}
									>
										{preview(entry.text)}
										{entry.runIds.length ? (
											<span className="text-xs text-ink-muted">{chatT("rewind.hasChanges")}</span>
										) : null}
									</button>
								</li>
							))}
						</ul>
					)}
				</div>
			</div>
		</div>
	);
}

/** Toast an error from a rewind action started outside the dialog. */
export function reportRewindError(err: unknown): void {
	showToast(err instanceof Error ? err.message : t("chat:rewind.failed"), "error");
}
