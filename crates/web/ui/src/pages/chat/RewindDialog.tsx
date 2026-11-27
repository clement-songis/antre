// ── Rewind dialog ───────────────────────────────────────────
//
// Opened from a user message's "Rewind" action (one turn) or with Esc Esc
// (pick a turn first). Offers restoring the conversation, the files the
// later turns changed, or both.

import { useEffect, useRef, useState } from "preact/hooks";
import { showToast } from "../../ui";
import { type RewindPoint, type RewindScope, rewindPickerOpen, rewindTarget, rewindTo, runIdsFrom } from "./rewind";

function preview(text: string): string {
	const line = text.trim().split("\n")[0] || "(attachment)";
	return line.length > 120 ? `${line.slice(0, 120)}…` : line;
}

interface ScopeChoiceProps {
	sessionKey: string;
	point: RewindPoint;
	points: RewindPoint[];
	onDone: () => void;
}

function ScopeChoice({ sessionKey, point, points, onDone }: ScopeChoiceProps) {
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
			setError(err instanceof Error ? err.message : "Rewind failed");
			setBusy(false);
		}
	};
	return (
		<div className="flex flex-col gap-2">
			<div className="text-xs text-[var(--muted)]">Rewind to before:</div>
			<blockquote className="rewind-quote m-0 text-sm border-l-2 border-[var(--border)] pl-2">
				{preview(point.text)}
			</blockquote>
			<div className="flex flex-col gap-1.5">
				<button ref={firstRef} type="button" className="provider-btn" disabled={busy} onClick={() => void run("both")}>
					Restore conversation and files
				</button>
				<button
					type="button"
					className="provider-btn provider-btn-secondary"
					disabled={busy}
					onClick={() => void run("conversation")}
				>
					Restore conversation only
				</button>
				<button
					type="button"
					className="provider-btn provider-btn-secondary"
					disabled={busy || !hasFileChanges}
					title={
						hasFileChanges ? "Undo file changes made after this message" : "No file checkpoints after this message"
					}
					onClick={() => void run("files")}
				>
					Restore files only
				</button>
			</div>
			<div className="text-xs text-[var(--muted)]">
				The conversation from this message on is removed; its text goes back to the composer.
			</div>
			{error ? (
				<div className="text-xs text-[var(--error)]" role="alert">
					{error}
				</div>
			) : null}
		</div>
	);
}

export function RewindDialog() {
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
				aria-label="Rewind"
			>
				<div className="provider-modal-header">
					<div className="provider-item-name">Rewind</div>
					<button type="button" className="provider-btn provider-btn-secondary provider-btn-sm" onClick={close}>
						Close
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
						<div className="text-sm text-[var(--muted)]">Nothing to rewind yet.</div>
					) : (
						<ul className="rewind-turns m-0 p-0 list-none flex flex-col gap-1 max-h-[50vh] overflow-y-auto">
							{[...points].reverse().map((entry) => (
								<li key={entry.index}>
									<button
										type="button"
										className="rewind-turn w-full text-left text-sm px-2 py-1.5 rounded-md border border-[var(--border)] bg-transparent cursor-pointer hover:bg-[var(--surface2)]"
										onClick={() => setPicked(entry)}
									>
										{preview(entry.text)}
										{entry.runIds.length ? (
											<span className="text-xs text-[var(--muted)]">{" · has changes"}</span>
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
	showToast(err instanceof Error ? err.message : "Rewind failed", "error");
}
