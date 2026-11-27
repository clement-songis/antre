// ── @file mentions in the composer ──────────────────────────
//
// Typing `@` followed by part of a path lists matching files of the
// session's project; picking one inserts `@relative/path ` so the agent
// knows which file is meant.

import { signal } from "@preact/signals";
import { sendRpc } from "../../helpers";
import * as S from "../../state";
import { sessionStore } from "../../stores/session-store";

interface MentionState {
	/** Index of the `@` in the composer text. */
	start: number;
	query: string;
	files: string[];
	active: number;
}

export const mentionState = signal<MentionState | null>(null);

const SEARCH_DEBOUNCE_MS = 120;
let searchTimer: ReturnType<typeof setTimeout> | null = null;
let searchGeneration = 0;

/** The `@query` the caret sits in, if any: `@` at the start or after whitespace. */
export function mentionAtCaret(text: string, caret: number): { start: number; query: string } | null {
	const before = text.slice(0, caret);
	const match = /(^|\s)@([^\s@]*)$/.exec(before);
	if (!match) return null;
	return { start: caret - match[2].length - 1, query: match[2] };
}

function closeMentions(): void {
	if (searchTimer) clearTimeout(searchTimer);
	searchTimer = null;
	searchGeneration += 1;
	mentionState.value = null;
}

function searchFiles(start: number, query: string): void {
	const session = sessionStore.activeSession.value;
	const projectId = session?.projectId;
	if (!projectId) {
		closeMentions();
		return;
	}
	if (searchTimer) clearTimeout(searchTimer);
	const generation = ++searchGeneration;
	searchTimer = setTimeout(() => {
		sendRpc<{ files?: string[] }>("projects.files", {
			projectId,
			sessionKey: session?.key,
			query,
			limit: 12,
		}).then((res) => {
			if (generation !== searchGeneration) return;
			const files = res?.ok ? res.payload?.files || [] : [];
			mentionState.value = files.length ? { start, query, files, active: 0 } : null;
		});
	}, SEARCH_DEBOUNCE_MS);
}

/** Call on every composer input. */
export function mentionHandleInput(): void {
	const input = S.chatInput as HTMLTextAreaElement | null;
	if (!input) return;
	const at = mentionAtCaret(input.value, input.selectionStart ?? input.value.length);
	if (!at) {
		if (mentionState.value) closeMentions();
		return;
	}
	searchFiles(at.start, at.query);
}

function insertMention(path: string): void {
	const input = S.chatInput as HTMLTextAreaElement | null;
	const state = mentionState.value;
	if (!(input && state)) return;
	const caret = input.selectionStart ?? input.value.length;
	const inserted = `@${path} `;
	input.value = input.value.slice(0, state.start) + inserted + input.value.slice(caret);
	const nextCaret = state.start + inserted.length;
	input.setSelectionRange(nextCaret, nextCaret);
	closeMentions();
	input.focus();
	input.dispatchEvent(new Event("input"));
}

/** Call first in the composer keydown handler; returns true when handled. */
export function mentionHandleKeydown(e: KeyboardEvent): boolean {
	const state = mentionState.value;
	if (!state) return false;
	if (e.key === "ArrowDown" || e.key === "ArrowUp") {
		e.preventDefault();
		const step = e.key === "ArrowDown" ? 1 : -1;
		mentionState.value = { ...state, active: (state.active + step + state.files.length) % state.files.length };
		return true;
	}
	if ((e.key === "Enter" && !e.shiftKey) || e.key === "Tab") {
		e.preventDefault();
		insertMention(state.files[state.active]);
		return true;
	}
	if (e.key === "Escape") {
		e.preventDefault();
		closeMentions();
		return true;
	}
	return false;
}

export function FileMentionMenu() {
	const state = mentionState.value;
	if (!state) return null;
	return (
		<div className="slash-menu file-mention-menu" role="listbox" aria-label="Files">
			{state.files.map((path, index) => {
				const slash = path.lastIndexOf("/");
				return (
					<div
						key={path}
						role="option"
						aria-selected={index === state.active}
						tabIndex={-1}
						className={`slash-menu-item${index === state.active ? " active" : ""}`}
						onMouseDown={(e) => {
							e.preventDefault();
							insertMention(path);
						}}
					>
						<span className="slash-name">{slash >= 0 ? path.slice(slash + 1) : path}</span>
						<span className="slash-desc">{slash >= 0 ? path.slice(0, slash) : ""}</span>
					</div>
				);
			})}
		</div>
	);
}

export function resetMentions(): void {
	closeMentions();
}
