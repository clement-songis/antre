// ── Building blocks shared by tool renderers ────────────────

import type { ComponentChildren } from "preact";
import type { ToolItem } from "../model";

export type JsonRecord = Record<string, unknown>;

export function isRecord(value: unknown): value is JsonRecord {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function str(value: unknown): string {
	return typeof value === "string" ? value : "";
}

export function num(value: unknown): number | null {
	return typeof value === "number" && Number.isFinite(value) ? value : null;
}

export function arg(item: ToolItem, ...keys: string[]): string {
	for (const key of keys) {
		const value = str(item.args[key]).trim();
		if (value) return value;
	}
	return "";
}

/** The tool result as an object; string results are wrapped as `{ text }`. */
export function resultRecord(item: ToolItem): JsonRecord | null {
	if (isRecord(item.result)) return item.result;
	if (typeof item.result === "string" && item.result) {
		try {
			const parsed: unknown = JSON.parse(item.result);
			if (isRecord(parsed)) return parsed;
		} catch {
			// Plain text result.
		}
		return { text: item.result };
	}
	return null;
}

/** Last path segments, so long absolute paths stay readable in a header. */
export function shortPath(path: string, segments = 3): string {
	const parts = path.split("/").filter(Boolean);
	if (parts.length <= segments) return path;
	return `…/${parts.slice(-segments).join("/")}`;
}

export function truncateText(text: string, max: number): { text: string; truncated: boolean } {
	if (text.length <= max) return { text, truncated: false };
	return { text: `${text.slice(0, max)}…`, truncated: true };
}

export function plural(count: number, word: string): string {
	return `${count} ${word}${count === 1 ? "" : "s"}`;
}

interface OutputProps {
	text: string;
	className?: string;
	maxChars?: number;
}

export function OutputBlock({ text, className = "", maxChars = 20_000 }: OutputProps) {
	if (!text) return null;
	const shown = truncateText(text, maxChars);
	return (
		<pre className={`exec-output ${className}`.trim()}>
			{shown.text}
			{shown.truncated ? (
				<span className="text-[var(--muted)]">{`\n[${text.length - maxChars} more characters]`}</span>
			) : null}
		</pre>
	);
}

export function JsonBlock({ value }: { value: unknown }) {
	if (value === undefined || value === null) return null;
	let text: string;
	try {
		text = typeof value === "string" ? value : JSON.stringify(value, null, 2);
	} catch {
		text = String(value);
	}
	return <OutputBlock text={text} className="tool-json" />;
}

export function Section({ label, children }: { label: string; children: ComponentChildren }) {
	return (
		<div className="tool-section">
			<div className="tool-section-label">{label}</div>
			{children}
		</div>
	);
}
