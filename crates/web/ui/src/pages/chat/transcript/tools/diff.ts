// ── Line diff for edit tool cards ───────────────────────────
//
// Edit arguments carry the replaced and the new text, so the card can show
// the change without reading the file. A longest-common-subsequence table
// is fine at this size; past MAX_CELLS the diff falls back to "all removed,
// all added" rather than freezing the page.

export type DiffLineKind = "context" | "add" | "del";

export interface DiffLine {
	kind: DiffLineKind;
	text: string;
}

export interface DiffStats {
	added: number;
	removed: number;
}

const MAX_CELLS = 250_000;

function splitLines(text: string): string[] {
	if (!text) return [];
	const lines = text.split("\n");
	if (lines[lines.length - 1] === "") lines.pop();
	return lines;
}

export function diffLines(before: string, after: string): DiffLine[] {
	const a = splitLines(before);
	const b = splitLines(after);
	if (a.length * b.length > MAX_CELLS) {
		return [...a.map((text) => ({ kind: "del" as const, text })), ...b.map((text) => ({ kind: "add" as const, text }))];
	}
	// lcs[i][j] = length of the common subsequence of a[i..] and b[j..].
	const lcs: Uint32Array[] = Array.from({ length: a.length + 1 }, () => new Uint32Array(b.length + 1));
	for (let i = a.length - 1; i >= 0; i--) {
		for (let j = b.length - 1; j >= 0; j--) {
			lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
		}
	}
	const out: DiffLine[] = [];
	let i = 0;
	let j = 0;
	while (i < a.length && j < b.length) {
		if (a[i] === b[j]) {
			out.push({ kind: "context", text: a[i] });
			i++;
			j++;
		} else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
			out.push({ kind: "del", text: a[i] });
			i++;
		} else {
			out.push({ kind: "add", text: b[j] });
			j++;
		}
	}
	while (i < a.length) out.push({ kind: "del", text: a[i++] });
	while (j < b.length) out.push({ kind: "add", text: b[j++] });
	return out;
}

export function diffStats(lines: DiffLine[]): DiffStats {
	let added = 0;
	let removed = 0;
	for (const line of lines) {
		if (line.kind === "add") added++;
		else if (line.kind === "del") removed++;
	}
	return { added, removed };
}

/**
 * Keep changed lines plus `context` lines around them; long unchanged runs
 * become a single `null` gap marker.
 */
export function collapseContext(lines: DiffLine[], context = 3): (DiffLine | null)[] {
	const keep = new Array<boolean>(lines.length).fill(false);
	lines.forEach((line, index) => {
		if (line.kind === "context") return;
		for (let k = Math.max(0, index - context); k <= Math.min(lines.length - 1, index + context); k++) keep[k] = true;
	});
	const out: (DiffLine | null)[] = [];
	let gap = false;
	lines.forEach((line, index) => {
		if (keep[index]) {
			out.push(line);
			gap = false;
		} else if (!gap) {
			out.push(null);
			gap = true;
		}
	});
	return out;
}
