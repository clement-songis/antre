// ── Tool call utilities ───────────────────────────────────────

import { renderDocument, renderMapLinks, renderMapPointGroups, renderScreenshot } from "../helpers";
import * as S from "../state";
import type { ToolResult } from "../types/ws-events";

export { stripChannelPrefix } from "../chat-ui";

// ── Tool result rendering ─────────────────────────────────────

function mediaUrl(sessionKey: string, path: string): string {
	const storedName = path.split("/").pop() || "";
	return `/api/sessions/${encodeURIComponent(sessionKey)}/media/${encodeURIComponent(storedName)}`;
}

/** Live results carry base64 data; persisted history carries a stored media path. */
function screenshotSrc(screenshot: string, sessionKey: string): string {
	if (screenshot.startsWith("data:")) return screenshot;
	const isStoredPath = screenshot.length < 512 && /\.(png|jpe?g|webp|gif)$/i.test(screenshot);
	return isStoredPath ? mediaUrl(sessionKey, screenshot) : `data:image/png;base64,${screenshot}`;
}

/** "text": stdout/stderr and exit code. "media": screenshot, document, map. */
export type ToolResultParts = "text" | "media" | "all";

/**
 * Render the generic parts of a tool result: stdout/stderr, exit code,
 * screenshot, document card and map links.
 */
export function appendToolResult(
	toolCard: HTMLElement,
	result: ToolResult,
	sessionKey: string,
	parts: ToolResultParts = "all",
): void {
	if (parts !== "media") appendTextResult(toolCard, result);
	if (parts !== "text") appendMediaResult(toolCard, result, sessionKey);
}

function appendTextResult(toolCard: HTMLElement, result: ToolResult): void {
	const out = (result.stdout || "").replace(/\n+$/, "");
	if (out) {
		const outEl = document.createElement("pre");
		outEl.className = "exec-output";
		outEl.textContent = out;
		toolCard.appendChild(outEl);
	}
	const stderrText = (result.stderr || "").replace(/\n+$/, "");
	if (stderrText) {
		const errEl = document.createElement("pre");
		errEl.className = "exec-output exec-stderr";
		errEl.textContent = stderrText;
		toolCard.appendChild(errEl);
	}
	if (result.exit_code !== undefined && result.exit_code !== 0) {
		const codeEl = document.createElement("div");
		codeEl.className = "exec-exit";
		codeEl.textContent = `exit ${result.exit_code}`;
		toolCard.appendChild(codeEl);
	}
}

function appendMediaResult(toolCard: HTMLElement, result: ToolResult, sessionKey: string): void {
	const key = sessionKey || S.activeSessionKey || "main";
	if (result.screenshot)
		renderScreenshot(toolCard, screenshotSrc(result.screenshot, key), result.screenshot_scale || 1);
	if (result.document_ref) {
		const storedName = result.document_ref.split("/").pop() || "";
		renderDocument(
			toolCard,
			mediaUrl(key, result.document_ref),
			result.filename || storedName,
			result.mime_type,
			result.size_bytes,
		);
	}
	const renderedPointGroups = renderMapPointGroups(toolCard, result.points, result.label);
	if (!renderedPointGroups && result.map_links) {
		renderMapLinks(toolCard, result.map_links, result.label);
	}
}

// ── Echo detection ────────────────────────────────────────────

function normalizeEchoComparable(text: string | null | undefined): string {
	if (!text) return "";
	return text
		.replace(/```[a-zA-Z0-9_-]*\n?/g, "")
		.replace(/```/g, "")
		.replace(/[`\s]/g, "");
}

/** True when the final reply only repeats the last tool's output. */
export function isPureToolOutputEcho(finalText: string, toolOutput: string): boolean {
	const finalComparable = normalizeEchoComparable(finalText);
	const toolComparable = normalizeEchoComparable(toolOutput);
	if (!(finalComparable && toolComparable)) return false;
	return finalComparable === toolComparable;
}
