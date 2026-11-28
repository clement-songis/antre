// ── Queued messages tray ────────────────────────────────────

import { useLayoutEffect, useState } from "preact/hooks";
import { useTranslation } from "../../i18n";
import { sessionStore } from "../../stores/session-store";
import { showToast } from "../../ui";
import { editQueued, type QueuedEntry, queuedForSession, queuedMessages, removeQueued, steerQueued } from "./queue";

function QueuedItem({ entry, first }: { entry: QueuedEntry; first: boolean }) {
	const { t } = useTranslation("chat");
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
				<span className="queued-label">{first ? t("queue.next") : t("queue.queued")}</span>
				<button
					type="button"
					className="queued-action"
					disabled={busy}
					title={t("queue.sendNowTitle")}
					onClick={() => void act(steerQueued, t("queue.sendNowFailed"))}
				>
					{t("queue.sendNow")}
				</button>
				<button
					type="button"
					className="queued-action"
					disabled={busy}
					title={t("queue.editTitle")}
					onClick={() => void act(editQueued, t("queue.editFailed"))}
				>
					{t("queue.edit")}
				</button>
				<button
					type="button"
					className="queued-cancel"
					disabled={busy}
					title={t("queue.remove")}
					aria-label={t("queue.remove")}
					onClick={() => void act(removeQueued, t("queue.removeFailed"))}
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
