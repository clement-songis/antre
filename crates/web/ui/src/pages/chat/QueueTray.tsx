// ── Queued messages tray ────────────────────────────────────

import { useLayoutEffect, useState } from "preact/hooks";
import { sessionStore } from "../../stores/session-store";
import { showToast } from "../../ui";
import { editQueued, type QueuedEntry, queuedForSession, queuedMessages, removeQueued, steerQueued } from "./queue";

function QueuedItem({ entry, first }: { entry: QueuedEntry; first: boolean }) {
	const [busy, setBusy] = useState(false);
	const act = async (action: (e: QueuedEntry) => Promise<boolean>, failure: string) => {
		setBusy(true);
		const ok = await action(entry);
		if (!ok) {
			showToast(failure, "error");
			setBusy(false);
		}
	};
	return (
		<div className="msg user queued" data-queued-seq={entry.seq}>
			<div className="queued-text">{entry.text}</div>
			<div className="queued-badge">
				<span className="queued-label">{first ? "Next" : "Queued"}</span>
				<button
					type="button"
					className="queued-action"
					disabled={busy}
					title="Stop the current reply and answer this message now"
					onClick={() => void act(steerQueued, "Could not send it now")}
				>
					Send now
				</button>
				<button
					type="button"
					className="queued-action"
					disabled={busy}
					title="Take it back to the composer"
					onClick={() => void act(editQueued, "Could not edit the queued message")}
				>
					Edit
				</button>
				<button
					type="button"
					className="queued-cancel"
					disabled={busy}
					title="Remove from the queue"
					aria-label="Remove from the queue"
					onClick={() => void act(removeQueued, "Could not remove the queued message")}
				>
					{"✕"}
				</button>
			</div>
		</div>
	);
}

interface QueueTrayProps {
	host: HTMLElement;
}

export function QueueTray({ host }: QueueTrayProps) {
	void queuedMessages.value;
	const entries = queuedForSession(sessionStore.activeSessionKey.value);
	useLayoutEffect(() => {
		host.classList.toggle("hidden", entries.length === 0);
	}, [entries.length, host]);
	return (
		<>
			{entries.map((entry, index) => (
				<QueuedItem key={entry.seq} entry={entry} first={index === 0} />
			))}
		</>
	);
}
