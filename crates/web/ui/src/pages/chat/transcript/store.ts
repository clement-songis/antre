// ── Transcript store (signals) ──────────────────────────────
//
// Holds the items of the session on screen. Every mutation goes through
// these functions; components only read `transcriptItems`. Items are
// addressed by id, never by position, so a placeholder removed later can
// never take another node with it.

import { batch, signal } from "@preact/signals";
import { THINKING_ITEM_ID, type TranscriptItem } from "./model";

export const transcriptItems = signal<TranscriptItem[]>([]);

/** True when the session has no message yet (shows the welcome card). */
export const transcriptEmpty = signal<boolean>(false);

export function getItem<K extends TranscriptItem["kind"]>(
	id: string,
	kind?: K,
): Extract<TranscriptItem, { kind: K }> | undefined {
	const item = transcriptItems.value.find((entry) => entry.id === id);
	if (!item || (kind && item.kind !== kind)) return undefined;
	return item as Extract<TranscriptItem, { kind: K }>;
}

export function hasItem(id: string): boolean {
	return transcriptItems.value.some((entry) => entry.id === id);
}

export function appendItem(item: TranscriptItem): string {
	const items = transcriptItems.value.filter((entry) => entry.id !== item.id);
	const thinkingIndex = items.findIndex((entry) => entry.id === THINKING_ITEM_ID);
	// The thinking indicator always stays last.
	if (thinkingIndex >= 0 && item.id !== THINKING_ITEM_ID) {
		items.splice(thinkingIndex, 0, item);
	} else {
		items.push(item);
	}
	batch(() => {
		transcriptItems.value = items;
		transcriptEmpty.value = false;
	});
	return item.id;
}

export function prependItems(items: TranscriptItem[]): void {
	if (items.length === 0) return;
	const ids = new Set(items.map((item) => item.id));
	transcriptItems.value = [...items, ...transcriptItems.value.filter((entry) => !ids.has(entry.id))];
}

export function updateItem<K extends TranscriptItem["kind"]>(
	id: string,
	kind: K,
	patch: (item: Extract<TranscriptItem, { kind: K }>) => Partial<Extract<TranscriptItem, { kind: K }>>,
): boolean {
	let changed = false;
	const next = transcriptItems.value.map((entry) => {
		if (entry.id !== id || entry.kind !== kind) return entry;
		changed = true;
		return { ...entry, ...patch(entry as Extract<TranscriptItem, { kind: K }>) } as TranscriptItem;
	});
	if (changed) transcriptItems.value = next;
	return changed;
}

export function removeItem(id: string): void {
	if (!hasItem(id)) return;
	transcriptItems.value = transcriptItems.value.filter((entry) => entry.id !== id);
}

export function removeItems(predicate: (item: TranscriptItem) => boolean): void {
	const next = transcriptItems.value.filter((entry) => !predicate(entry));
	if (next.length !== transcriptItems.value.length) transcriptItems.value = next;
}

export function replaceItems(items: TranscriptItem[]): void {
	batch(() => {
		transcriptItems.value = items;
		transcriptEmpty.value = false;
	});
}

export function clearTranscript(): void {
	cancelPendingDeltas();
	batch(() => {
		transcriptItems.value = [];
		transcriptEmpty.value = false;
	});
}

export function lastItem(): TranscriptItem | undefined {
	const items = transcriptItems.value;
	for (let index = items.length - 1; index >= 0; index--) {
		if (items[index].id !== THINKING_ITEM_ID) return items[index];
	}
	return undefined;
}

// ── Delta batching ───────────────────────────────────────────
// Streaming deltas arrive far faster than the screen refreshes. They are
// buffered per item and applied once per animation frame.

const pendingDeltas = new Map<string, string>();
let frameHandle: number | null = null;

function flushDeltas(): void {
	frameHandle = null;
	if (pendingDeltas.size === 0) return;
	const deltas = new Map(pendingDeltas);
	pendingDeltas.clear();
	transcriptItems.value = transcriptItems.value.map((entry) => {
		const delta = deltas.get(entry.id);
		if (delta === undefined || entry.kind !== "assistant") return entry;
		return { ...entry, text: entry.text + delta };
	});
}

function scheduleFrame(): void {
	if (frameHandle !== null) return;
	if (typeof requestAnimationFrame === "function" && !document.hidden) {
		frameHandle = requestAnimationFrame(flushDeltas);
	} else {
		frameHandle = window.setTimeout(flushDeltas, 16);
	}
}

export function queueAssistantDelta(id: string, delta: string): void {
	pendingDeltas.set(id, (pendingDeltas.get(id) || "") + delta);
	scheduleFrame();
}

/** Apply buffered deltas now (before a final/abort replaces the text). */
export function flushPendingDeltas(): void {
	if (frameHandle !== null) {
		if (typeof cancelAnimationFrame === "function") cancelAnimationFrame(frameHandle);
		clearTimeout(frameHandle);
	}
	flushDeltas();
}

export function cancelPendingDeltas(): void {
	pendingDeltas.clear();
	if (frameHandle !== null) {
		if (typeof cancelAnimationFrame === "function") cancelAnimationFrame(frameHandle);
		clearTimeout(frameHandle);
		frameHandle = null;
	}
}

/** Text of an assistant item including deltas not painted yet. */
export function assistantTextWithPending(id: string): string {
	const item = getItem(id, "assistant");
	return (item?.text || "") + (pendingDeltas.get(id) || "");
}
