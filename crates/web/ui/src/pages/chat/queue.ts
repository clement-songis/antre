// ── Queued messages ─────────────────────────────────────────
//
// A message sent while a reply runs is queued by the server and replayed
// when the run ends. The client keeps its own copy (keyed by the `_seq` it
// sent) to show the tray, and moves the first one into the transcript when
// the server starts replaying it.

import { signal } from "@preact/signals";
import { sendRpc } from "../../helpers";
import * as S from "../../state";
import { addUserMessage } from "./transcript/actions";

export interface QueuedEntry {
	seq: number;
	sessionKey: string;
	text: string;
}

export const queuedMessages = signal<QueuedEntry[]>([]);

export function queuedForSession(sessionKey: string): QueuedEntry[] {
	return queuedMessages.value.filter((entry) => entry.sessionKey === sessionKey);
}

export function enqueueMessage(entry: QueuedEntry): void {
	queuedMessages.value = [
		...queuedMessages.value.filter((e) => !(e.sessionKey === entry.sessionKey && e.seq === entry.seq)),
		entry,
	];
}

function removeEntry(sessionKey: string, seq: number): QueuedEntry | undefined {
	const entry = queuedMessages.value.find((e) => e.sessionKey === sessionKey && e.seq === seq);
	if (entry) queuedMessages.value = queuedMessages.value.filter((e) => e !== entry);
	return entry;
}

export function clearQueuedMessages(sessionKey: string): void {
	queuedMessages.value = queuedMessages.value.filter((entry) => entry.sessionKey !== sessionKey);
}

/** Another client (or this one) removed a queued message on the server. */
export function handleQueueItemRemoved(sessionKey: string, seq: number): void {
	removeEntry(sessionKey, seq);
}

/** The server replays the oldest queued message: show it as sent. */
export function moveFirstQueuedToTranscript(sessionKey: string): void {
	const first = queuedForSession(sessionKey)[0];
	if (!first) return;
	removeEntry(sessionKey, first.seq);
	addUserMessage({ text: first.text });
}

async function cancelOnServer(entry: QueuedEntry): Promise<boolean> {
	const res = await sendRpc("chat.cancel_queued", { sessionKey: entry.sessionKey, seq: entry.seq });
	return !!res?.ok;
}

export async function removeQueued(entry: QueuedEntry): Promise<boolean> {
	if (!(await cancelOnServer(entry))) return false;
	removeEntry(entry.sessionKey, entry.seq);
	return true;
}

/** Take the message back into the composer to change it before sending. */
export async function editQueued(entry: QueuedEntry): Promise<boolean> {
	if (!(await removeQueued(entry))) return false;
	const input = S.chatInput as HTMLTextAreaElement | null;
	if (input) {
		input.value = entry.text;
		input.dispatchEvent(new Event("input"));
		input.focus();
	}
	return true;
}

/** Stop the running reply and answer this message next. */
export async function steerQueued(entry: QueuedEntry): Promise<boolean> {
	const res = await sendRpc("chat.steer", { sessionKey: entry.sessionKey, seq: entry.seq });
	if (!res?.ok) return false;
	const rest = queuedMessages.value.filter((e) => e !== entry);
	queuedMessages.value = [entry, ...rest];
	return true;
}
