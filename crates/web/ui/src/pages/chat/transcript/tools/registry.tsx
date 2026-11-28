// ── Per-tool renderer registry ──────────────────────────────
//
// Each renderer turns a tool call into a one-line header (icon, verb,
// target, badge) and an expandable body. Live events and reloaded history
// produce the same `ToolItem`, so both go through the same renderer.

import type { ComponentChildren } from "preact";
import { toolCallSummary } from "../../../../helpers";
import { t } from "../../../../i18n";
import { Markdown } from "../Markdown";
import type { ToolItem } from "../model";
import { McpAppView, mcpAppUri, mcpToolParts, OpenDesignButton } from "./apps";
import { collapseContext, type DiffLine, diffLines, diffStats } from "./diff";
import {
	arg,
	type CountUnit,
	countLabel,
	isRecord,
	JsonBlock,
	type JsonRecord,
	num,
	OutputBlock,
	resultRecord,
	Section,
	shortPath,
	str,
	truncateText,
} from "./shared";
import { TaskChecklist, tasksFromToolItem } from "./tasks";

export interface ToolView {
	/** Icon class from the mask icon set (`icon-terminal`, ...). */
	icon: string;
	verb: string;
	/** What the call acted on: a path, a command, a URL. Rendered monospace. */
	target?: string;
	/** Short result summary shown in the header (`+12 −3`, `4 matches`). */
	badge?: ComponentChildren;
	/** Expanded content; generic stdout/screenshot/document output is added after it. */
	body?: ComponentChildren;
	/** Show the body without waiting for a click (live checklists). */
	openByDefault?: boolean;
}

type Renderer = (item: ToolItem) => ToolView;

// ── Edits and diffs ──────────────────────────────────────────

function DiffView({ lines }: { lines: DiffLine[] }) {
	const shown = collapseContext(lines);
	return (
		<pre className="tool-diff">
			{shown.map((line, index) =>
				line === null ? (
					// biome-ignore lint/suspicious/noArrayIndexKey: gap markers have no identity.
					<span key={`gap-${index}`} className="tool-diff-gap">
						{"⋯\n"}
					</span>
				) : (
					// biome-ignore lint/suspicious/noArrayIndexKey: lines are positional.
					<span key={index} className={`tool-diff-line tool-diff-${line.kind}`}>
						{`${line.kind === "add" ? "+" : line.kind === "del" ? "-" : " "} ${line.text}\n`}
					</span>
				),
			)}
		</pre>
	);
}

function DiffBadge({ added, removed }: { added: number; removed: number }) {
	return (
		<span className="tool-diff-badge">
			<span className="tool-diff-badge-add">{`+${added}`}</span>{" "}
			<span className="tool-diff-badge-del">{`−${removed}`}</span>
		</span>
	);
}

interface EditPair {
	before: string;
	after: string;
}

function editPairs(item: ToolItem): EditPair[] {
	if (item.name === "MultiEdit") {
		const edits = Array.isArray(item.args.edits) ? item.args.edits : [];
		return edits.filter(isRecord).map((edit) => ({ before: str(edit.old_string), after: str(edit.new_string) }));
	}
	if (item.name === "Write") return [{ before: "", after: str(item.args.content) }];
	return [{ before: str(item.args.old_string), after: str(item.args.new_string) }];
}

const renderEdit: Renderer = (item) => {
	const path = arg(item, "file_path", "path");
	const diffs = editPairs(item).map((pair) => diffLines(pair.before, pair.after));
	const stats = diffs.reduce(
		(acc, lines) => {
			const s = diffStats(lines);
			return { added: acc.added + s.added, removed: acc.removed + s.removed };
		},
		{ added: 0, removed: 0 },
	);
	const verb =
		item.name === "Write"
			? item.status === "running"
				? t("chat:tools.writing")
				: t("chat:tools.wrote")
			: item.status === "running"
				? t("chat:tools.editing")
				: t("chat:tools.edited");
	const replaceAll = item.args.replace_all === true;
	return {
		icon: "icon-code",
		verb,
		target: shortPath(path),
		badge: <DiffBadge added={stats.added} removed={stats.removed} />,
		body: (
			<>
				{path ? (
					<div className="tool-path" title={path}>
						{path}
						{replaceAll ? t("chat:tools.allOccurrences") : ""}
					</div>
				) : null}
				{diffs.map((lines, index) => (
					// biome-ignore lint/suspicious/noArrayIndexKey: edits are positional.
					<DiffView key={index} lines={lines} />
				))}
			</>
		),
	};
};

// ── Files and search ─────────────────────────────────────────

const renderRead: Renderer = (item) => {
	const path = arg(item, "file_path", "path");
	const res = resultRecord(item);
	const offset = num(item.args.offset);
	const limit = num(item.args.limit);
	const total = num(res?.total_lines);
	const range =
		offset || limit
			? t("chat:tools.lines", { range: `${offset ?? 1}–${(offset ?? 1) + (limit ?? 0) - (limit ? 1 : 0) || ""}` })
			: "";
	const content = str(res?.content);
	return {
		icon: "icon-file",
		verb: item.status === "running" ? t("chat:tools.reading") : t("chat:tools.read"),
		target: shortPath(path) + range,
		badge: total === null ? undefined : countLabel(total, "line"),
		body: (
			<>
				{path ? (
					<div className="tool-path" title={path}>
						{path}
					</div>
				) : null}
				<OutputBlock text={content} maxChars={8_000} />
			</>
		),
	};
};

function stringList(value: unknown): string[] {
	return Array.isArray(value)
		? value.map((entry) => (typeof entry === "string" ? entry : str(isRecord(entry) ? entry.path : ""))).filter(Boolean)
		: [];
}

const renderGlob: Renderer = (item) => {
	const res = resultRecord(item);
	const paths = stringList(res?.paths);
	const where = arg(item, "path");
	return {
		icon: "icon-search",
		verb: "Glob",
		target: where
			? t("chat:tools.inPath", { pattern: arg(item, "pattern"), path: shortPath(where) })
			: arg(item, "pattern"),
		badge: res ? `${countLabel(paths.length, "file")}${res.truncated ? "+" : ""}` : undefined,
		body: <OutputBlock text={paths.join("\n")} />,
	};
};

function grepLines(res: JsonRecord | null): { lines: string[]; count: number; unit: "match" | "file" } {
	if (!res) return { lines: [], count: 0, unit: "match" };
	if (Array.isArray(res.matches)) {
		const lines = res.matches.filter(isRecord).map((m) => `${str(m.path)}:${num(m.line) ?? ""}: ${str(m.match)}`);
		return { lines, count: lines.length, unit: "match" };
	}
	if (Array.isArray(res.counts)) {
		const lines = res.counts.filter(isRecord).map((c) => `${str(c.path)}: ${num(c.count) ?? 0}`);
		return { lines, count: lines.length, unit: "file" };
	}
	const files = stringList(res.files);
	return { lines: files, count: files.length, unit: "file" };
}

const renderGrep: Renderer = (item) => {
	const res = resultRecord(item);
	const { lines, count, unit } = grepLines(res);
	const where = arg(item, "path");
	return {
		icon: "icon-search",
		verb: "Grep",
		target: where
			? t("chat:tools.inPath", { pattern: arg(item, "pattern"), path: shortPath(where) })
			: arg(item, "pattern"),
		badge: res ? `${countLabel(count, unit)}${res.truncated ? "+" : ""}` : undefined,
		body: <OutputBlock text={lines.join("\n")} />,
	};
};

// ── Shell ────────────────────────────────────────────────────

const renderExec: Renderer = (item) => {
	const res = resultRecord(item);
	const exit = num(res?.exit_code);
	return {
		icon: "icon-terminal",
		verb: item.status === "running" ? t("chat:tools.runningCmd") : t("chat:tools.ran"),
		target: toolCallSummary("exec", item.args as Parameters<typeof toolCallSummary>[1]),
		badge: exit !== null && exit !== 0 ? t("chat:tools.exit", { code: exit }) : undefined,
		body: (
			<div className="exec-prompt">
				<span className="exec-prompt-char">$</span>
				<span data-cmd>{` ${arg(item, "command")}`}</span>
			</div>
		),
	};
};

// ── Web and browser ──────────────────────────────────────────

const renderWebFetch: Renderer = (item) => {
	const res = resultRecord(item);
	const content = str(res?.content);
	return {
		icon: "icon-globe",
		verb: item.status === "running" ? t("chat:tools.fetching") : t("chat:tools.fetched"),
		target: arg(item, "url") || str(res?.url),
		badge: res?.truncated ? t("chat:tools.truncated") : undefined,
		body: <OutputBlock text={content} maxChars={6_000} />,
	};
};

const renderWebSearch: Renderer = (item) => {
	const res = resultRecord(item);
	const results = Array.isArray(res?.results) ? res.results.filter(isRecord) : [];
	return {
		icon: "icon-search",
		verb: t("chat:tools.searched"),
		target: `“${arg(item, "query")}”`,
		badge: res ? countLabel(results.length, "result") : undefined,
		body:
			results.length > 0 ? (
				<ul className="tool-link-list">
					{results.map((r, index) => {
						const url = str(r.url) || str(r.link);
						return (
							// biome-ignore lint/suspicious/noArrayIndexKey: results are positional.
							<li key={index}>
								{/^https?:/i.test(url) ? (
									<a href={url} target="_blank" rel="noopener noreferrer">
										{str(r.title) || url}
									</a>
								) : (
									<span>{str(r.title) || url}</span>
								)}
								{str(r.description) || str(r.snippet) ? (
									<div className="text-ink-muted text-xs">
										{truncateText(str(r.description) || str(r.snippet), 240).text}
									</div>
								) : null}
							</li>
						);
					})}
				</ul>
			) : null,
	};
};

const renderBrowser: Renderer = (item) => ({
	icon: "icon-monitor",
	verb: t("chat:tools.browser", { action: arg(item, "action") }).trim(),
	target: arg(item, "url", "selector", "text"),
	body: null,
});

// ── Agents and tasks ─────────────────────────────────────────

const renderSpawnAgent: Renderer = (item) => {
	const res = resultRecord(item);
	const task = arg(item, "task");
	const status = str(res?.status);
	const output = str(res?.result) || str(res?.output) || str(res?.text);
	return {
		icon: "icon-users",
		verb: item.status === "running" ? t("chat:tools.delegating") : t("chat:tools.subAgent"),
		target: truncateText(task.split("\n")[0] || "", 90).text,
		badge: status || (str(res?.model) ? str(res?.model) : undefined),
		body: (
			<>
				{task ? (
					<Section label={t("chat:tools.task")}>
						<Markdown text={task} />
					</Section>
				) : null}
				{arg(item, "context") ? (
					<Section label={t("chat:tools.context")}>
						<Markdown text={arg(item, "context")} />
					</Section>
				) : null}
				{output ? (
					<Section label={t("chat:tools.result")}>
						<Markdown text={output} />
					</Section>
				) : res ? (
					<JsonBlock value={res} />
				) : null}
			</>
		),
	};
};

const renderTaskList: Renderer = (item) => {
	const tasks = tasksFromToolItem(item);
	const action = arg(item, "action");
	const subject = arg(item, "subject");
	return {
		icon: "icon-check-circle",
		verb:
			action === "create"
				? t("chat:tools.addedTask")
				: action === "update"
					? t("chat:tools.updatedTask")
					: t("chat:tools.tasks"),
		target: subject || (tasks.length ? countLabel(tasks.length, "task") : action),
		badge: arg(item, "status") || undefined,
		body: tasks.length ? <TaskChecklist tasks={tasks} /> : <JsonBlock value={item.result} />,
	};
};

// ── MCP and fallback ─────────────────────────────────────────

const renderMcp: Renderer = (item) => {
	const parts = mcpToolParts(item.name);
	const server = parts?.server ?? "";
	const tool = parts?.tool ?? item.name;
	const appUri = item.status === "ok" ? mcpAppUri(item) : null;
	return {
		icon: "icon-connectors",
		verb: tool,
		target: server ? t("chat:tools.via", { server }) : undefined,
		openByDefault: appUri !== null,
		body: (
			<>
				{appUri ? <McpAppView item={item} uri={appUri} /> : null}
				<Section label={t("chat:tools.arguments")}>
					<JsonBlock value={item.args} />
				</Section>
				{item.result === undefined ? null : (
					<Section label={t("chat:tools.result")}>
						<JsonBlock value={item.result} />
					</Section>
				)}
			</>
		),
	};
};

/** Keys the generic stdout/screenshot/document output already shows. */
const GENERIC_RESULT_KEYS = new Set([
	"stdout",
	"stderr",
	"exit_code",
	"screenshot",
	"screenshot_scale",
	"document_ref",
	"filename",
	"mime_type",
	"size_bytes",
	"points",
	"label",
	"map_links",
]);

const renderDefault: Renderer = (item) => {
	const res = resultRecord(item);
	const extraResult = res && Object.keys(res).some((key) => !GENERIC_RESULT_KEYS.has(key));
	const hasArgs = Object.keys(item.args).some((key) => !key.startsWith("_"));
	return {
		icon: "icon-wrench",
		verb: item.name,
		target: toolCallSummary(item.name, item.args as Parameters<typeof toolCallSummary>[1], item.executionMode)
			.replace(item.name, "")
			.trim(),
		body: (
			<>
				{hasArgs ? (
					<Section label={t("chat:tools.arguments")}>
						<JsonBlock value={item.args} />
					</Section>
				) : null}
				{extraResult ? (
					<Section label={t("chat:tools.result")}>
						<JsonBlock value={res} />
					</Section>
				) : null}
			</>
		),
	};
};

// ── Design canvas ────────────────────────────────────────────

function designIdOf(item: ToolItem): string {
	const res = resultRecord(item);
	return str(res?.design_id) || arg(item, "design_id");
}

function renderDesign(verbKey: string): Renderer {
	return (item) => {
		const res = resultRecord(item);
		const designId = designIdOf(item);
		const version = num(res?.version);
		return {
			icon: "icon-layers",
			verb: t(verbKey),
			target: str(res?.title) || arg(item, "title") || (designId ? designId.slice(0, 8) : undefined),
			badge: version ? `v${version}` : undefined,
			body: designId && item.status === "ok" ? <OpenDesignButton designId={designId} /> : undefined,
		};
	};
}

const RENDERERS: Record<string, Renderer> = {
	design_create: renderDesign("canvas:createdDesign"),
	design_open: renderDesign("canvas:openedDesign"),
	design_screenshot: renderDesign("canvas:designScreenshot"),
	exec: renderExec,
	Read: renderRead,
	Write: renderEdit,
	Edit: renderEdit,
	MultiEdit: renderEdit,
	Glob: renderGlob,
	Grep: renderGrep,
	web_fetch: renderWebFetch,
	firecrawl_scrape: renderWebFetch,
	web_search: renderWebSearch,
	browser: renderBrowser,
	spawn_agent: renderSpawnAgent,
	task_list: renderTaskList,
};

export function toolView(item: ToolItem): ToolView {
	const renderer = RENDERERS[item.name] || (item.name.startsWith("mcp__") ? renderMcp : renderDefault);
	return renderer(item);
}

/** Tools whose header names a file; used to summarise a group of calls. */
export function toolNoun(item: ToolItem): CountUnit {
	switch (item.name) {
		case "exec":
			return "command";
		case "Read":
			return "read";
		case "Write":
		case "Edit":
		case "MultiEdit":
			return "edit";
		case "Glob":
		case "Grep":
		case "web_search":
			return "search";
		default:
			return item.name.startsWith("mcp__") ? "mcp" : "tool";
	}
}
