// ── Collapsible tool call card and groups ───────────────────

import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import { contextMenuTrigger, MenuButton } from "../../../../components/ContextMenu";
import { t } from "../../../../i18n";
import { navigate } from "../../../../router";
import type { ToolResult } from "../../../../types/ws-events";
import { appendToolResult, type ToolResultParts } from "../../../../ws/tool-helpers";
import { ReasoningDisclosure, UsageFooter } from "../Messages";
import { toolCardMenu } from "../message-menu";
import type { ToolItem } from "../model";
import { toolNoun, toolView } from "./registry";
import { type CountUnit, countLabel, isRecord } from "./shared";

const VALIDATION_ERROR_MARKERS = ["missing field", "missing required", "missing 'action'", "missing 'url'"];

/** A failed call whose arguments were malformed: the model retries it, so it is shown muted. */
export function isValidationError(item: ToolItem): boolean {
	if (item.status !== "error" || !item.error) return false;
	const detail = item.error.toLowerCase();
	return VALIDATION_ERROR_MARKERS.some((marker) => detail.includes(marker));
}

function stateClass(item: ToolItem): string {
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

export function formatElapsed(ms: number): string {
	if (ms < 1000) return `${Math.max(0, Math.round(ms))}ms`;
	const secs = ms / 1000;
	if (secs < 60) return `${secs < 10 ? secs.toFixed(1) : Math.round(secs)}s`;
	const mins = Math.floor(secs / 60);
	return `${mins}m ${Math.round(secs % 60)}s`;
}

/** Re-render every `intervalMs` while `active`, for live elapsed time. */
function useTicker(active: boolean, intervalMs = 500): number {
	const [now, setNow] = useState(Date.now());
	useEffect(() => {
		if (!active) return;
		const timer = setInterval(() => setNow(Date.now()), intervalMs);
		return () => clearInterval(timer);
	}, [active, intervalMs]);
	return now;
}

function elapsedLabel(item: ToolItem, now: number): string {
	if (!item.startedAt) return "";
	const end = item.endedAt ?? (item.status === "running" ? now : undefined);
	return end === undefined ? "" : formatElapsed(end - item.startedAt);
}

interface GenericResultProps {
	item: ToolItem;
	sessionKey: string;
	parts: ToolResultParts;
}

/** stdout/stderr (in the collapsible body) or deliverables (always shown), for every tool. */
function GenericResult({ item, sessionKey, parts }: GenericResultProps) {
	const ref = useRef<HTMLDivElement>(null);
	useLayoutEffect(() => {
		const el = ref.current;
		if (!el) return;
		el.textContent = "";
		if (item.status === "ok" && isRecord(item.result))
			appendToolResult(el, item.result as ToolResult, sessionKey, parts);
	}, [item.result, item.status, sessionKey, parts]);
	return <div ref={ref} className="contents" />;
}

function SkillHint({ name }: { name: string }) {
	return (
		<div className="skill-hint">
			{name === "create_skill" ? t("chat:tools.skillCreated") : t("chat:tools.skillUpdated")}
			<a
				href="/skills"
				onClick={(e) => {
					e.preventDefault();
					navigate("/skills");
				}}
			>
				{t("chat:tools.personalSkills")}
			</a>
		</div>
	);
}

const STATUS_ICON: Record<string, string> = {
	running: "",
	"exec-ok": "icon-checkmark",
	"exec-err": "icon-x-circle",
	"exec-retry": "icon-retry",
	"exec-cancelled": "icon-stop",
};

interface ToolCallCardProps {
	item: ToolItem;
	sessionKey: string;
}

export function ToolCallCard({ item, sessionKey }: ToolCallCardProps) {
	const view = toolView(item);
	const state = stateClass(item);
	const failed = state === "exec-err";
	const [open, setOpen] = useState<boolean | null>(null);
	// Failures open themselves so the error is seen; the user's toggle wins.
	const expanded = open ?? (view.openByDefault === true || failed);
	const now = useTicker(item.status === "running");
	const elapsed = elapsedLabel(item, now);
	const bodyId = `${item.id}-body`;
	return (
		<div
			className={`msg exec-card tool-card ${state}${expanded ? " expanded" : ""}`}
			id={item.id}
			{...contextMenuTrigger(() => toolCardMenu(item))}
			data-tool-name={item.name}
			data-tool-state={item.status}
			data-history-index={item.historyIndex}
		>
			<div className="tool-card-head-row">
				<button
					type="button"
					className="tool-card-header"
					aria-expanded={expanded}
					aria-controls={bodyId}
					onClick={() => setOpen(!expanded)}
				>
					<span className={`icon icon-sm ${view.icon} tool-card-icon`} aria-hidden="true" />
					<span className="tool-card-verb">{view.verb}</span>
					{view.target ? (
						<span className="tool-card-target" title={view.target}>
							{view.target}
						</span>
					) : null}
					{view.badge ? <span className="tool-card-badge">{view.badge}</span> : null}
					<span className="tool-card-meta">
						{item.status === "running" ? <span className="exec-status">{t("chat:tools.running")}</span> : null}
						{elapsed ? <span className="tool-card-elapsed">{elapsed}</span> : null}
						{STATUS_ICON[state] ? (
							<span className={`icon icon-xs ${STATUS_ICON[state]} tool-card-status`} aria-hidden="true" />
						) : null}
						<span className="icon icon-xs icon-chevron-right tool-card-chevron" aria-hidden="true" />
					</span>
				</button>
				<MenuButton className="tool-card-menu-btn" label={t("chat:tools.actions")} getMenu={() => toolCardMenu(item)} />
			</div>
			<div className="tool-card-body" id={bodyId} hidden={!expanded}>
				{view.body}
				<GenericResult item={item} sessionKey={sessionKey} parts="text" />
				{item.status === "error" && item.error ? (
					<div className={isValidationError(item) ? "exec-retry-detail" : "exec-error-detail"}>{item.error}</div>
				) : null}
			</div>
			<div className="tool-card-media">
				<GenericResult item={item} sessionKey={sessionKey} parts="media" />
			</div>
			{item.reasoning ? (
				<div className="tool-card-reasoning">
					<ReasoningDisclosure text={item.reasoning} />
				</div>
			) : null}
			{item.status === "cancelled" ? <div className="exec-cancelled-detail">Cancelled</div> : null}
			{item.status === "ok" && (item.name === "create_skill" || item.name === "update_skill") ? (
				<SkillHint name={item.name} />
			) : null}
			{item.turnFooter ? <UsageFooter {...item.turnFooter} showTime={true} /> : null}
		</div>
	);
}

function groupSummary(items: ToolItem[]): string {
	const counts = new Map<CountUnit, number>();
	for (const item of items) {
		const noun = toolNoun(item);
		counts.set(noun, (counts.get(noun) || 0) + 1);
	}
	return [...counts.entries()].map(([noun, count]) => countLabel(count, noun)).join(", ");
}

interface ToolGroupProps {
	items: ToolItem[];
	sessionKey: string;
}

/** Consecutive tool calls with no text between them, folded into one line. */
export function ToolGroup({ items, sessionKey }: ToolGroupProps) {
	const running = items.some((item) => item.status === "running");
	const failed = items.some((item) => item.status === "error" && !isValidationError(item));
	const [open, setOpen] = useState<boolean | null>(null);
	const expanded = open ?? (running || failed);
	const startedAt = items.find((item) => item.startedAt)?.startedAt;
	const endedAt = running ? undefined : Math.max(...items.map((item) => item.endedAt ?? 0));
	const now = useTicker(running);
	const elapsed = startedAt ? formatElapsed((endedAt || now) - startedAt) : "";
	return (
		<div
			className={`tool-group${expanded ? " expanded" : ""}${running ? " running" : ""}`}
			data-tool-count={items.length}
		>
			<button type="button" className="tool-group-header" aria-expanded={expanded} onClick={() => setOpen(!expanded)}>
				<span className="icon icon-xs icon-chevron-right tool-card-chevron" aria-hidden="true" />
				<span className="tool-group-summary">{groupSummary(items)}</span>
				{running ? <span className="exec-status">{t("chat:tools.running")}</span> : null}
				{elapsed ? <span className="tool-card-elapsed">{elapsed}</span> : null}
			</button>
			<div className="tool-group-body" hidden={!expanded}>
				{items.map((item) => (
					<ToolCallCard key={item.id} item={item} sessionKey={sessionKey} />
				))}
			</div>
		</div>
	);
}
