// ── Tool call card ──────────────────────────────────────────

import { useLayoutEffect, useRef } from "preact/hooks";
import { toolCallSummary } from "../../../helpers";
import { navigate } from "../../../router";
import type { ToolResult } from "../../../types/ws-events";
import { appendToolResult } from "../../../ws/tool-helpers";
import { ReasoningDisclosure, UsageFooter } from "./Messages";
import type { ToolItem } from "./model";

const VALIDATION_ERROR_MARKERS = ["missing field", "missing required", "missing 'action'", "missing 'url'"];

/** A failed call whose arguments were malformed: the model retries it, so it is shown muted. */
export function isValidationError(item: ToolItem): boolean {
	if (item.status !== "error" || !item.error) return false;
	const detail = item.error.toLowerCase();
	return VALIDATION_ERROR_MARKERS.some((marker) => detail.includes(marker));
}

export function toolCardStateClass(item: ToolItem): string {
	switch (item.status) {
		case "running":
			return "running";
		case "ok":
			return "exec-ok";
		case "cancelled":
			return "exec-cancelled";
		case "retry":
			return "exec-retry";
		default:
			return isValidationError(item) ? "exec-retry" : "exec-err";
	}
}

function ResultSlot({ item, sessionKey }: { item: ToolItem; sessionKey: string }) {
	const ref = useRef<HTMLDivElement>(null);
	useLayoutEffect(() => {
		const el = ref.current;
		if (!el) return;
		el.textContent = "";
		if (item.status === "ok" && item.result && typeof item.result === "object") {
			appendToolResult(el, item.result as ToolResult, sessionKey);
		}
	}, [item.result, item.status, sessionKey]);
	return <div ref={ref} className="contents" />;
}

function SkillHint({ name }: { name: string }) {
	const verb = name === "create_skill" ? "created" : "updated";
	return (
		<div className="skill-hint">
			{`Skill ${verb} — available in your `}
			<a
				href="/skills"
				onClick={(e) => {
					e.preventDefault();
					navigate("/skills");
				}}
			>
				personal skills
			</a>
		</div>
	);
}

interface ToolCardProps {
	item: ToolItem;
	sessionKey: string;
}

export function ToolCard({ item, sessionKey }: ToolCardProps) {
	const summary = toolCallSummary(item.name, item.args as Parameters<typeof toolCallSummary>[1], item.executionMode);
	const validation = isValidationError(item);
	return (
		<div
			className={`msg exec-card ${toolCardStateClass(item)}`}
			id={item.id}
			data-tool-name={item.name}
			data-tool-state={item.status}
			data-history-index={item.historyIndex}
		>
			<div className="exec-prompt">
				<span className="exec-prompt-char">$</span>
				<span data-cmd>{` ${summary}`}</span>
			</div>
			{item.status === "running" ? <div className="exec-status">running{"…"}</div> : null}
			<ResultSlot item={item} sessionKey={sessionKey} />
			{item.status === "error" && item.error ? (
				<div className={validation ? "exec-retry-detail" : "exec-error-detail"}>{item.error}</div>
			) : null}
			{item.status === "cancelled" ? <div className="exec-cancelled-detail">Cancelled</div> : null}
			{item.status === "ok" && (item.name === "create_skill" || item.name === "update_skill") ? (
				<SkillHint name={item.name} />
			) : null}
			{item.reasoning ? <ReasoningDisclosure text={item.reasoning} /> : null}
			{item.turnFooter ? <UsageFooter {...item.turnFooter} showTime={true} /> : null}
		</div>
	);
}
