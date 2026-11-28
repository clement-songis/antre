// ── MCP App views and design tool cards ─────────────────────
//
// An MCP tool whose definition names a `ui://` resource gets its view shown
// inline, through the same render-origin frame host as designs.

import { signal } from "@preact/signals";
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { sendRpc } from "../../../../helpers";
import { locale, useTranslation } from "../../../../i18n";
import { stageFrame } from "../../canvas/api";
import { type HostContext, handleViewMessage, parseJsonRpc } from "../../canvas/mcp-app-host";
import { SandboxFrame } from "../../canvas/SandboxFrame";
import { openDesign } from "../../canvas/store";
import type { FrameNetwork } from "../../canvas/types";
import type { ToolItem } from "../model";
import { isRecord, str } from "./shared";

/** `mcp__<server>__<tool>` split into its parts. */
export function mcpToolParts(name: string): { server: string; tool: string } | null {
	if (!name.startsWith("mcp__")) return null;
	const rest = name.slice(5);
	const split = rest.indexOf("__");
	if (split <= 0) return null;
	return { server: rest.slice(0, split), tool: rest.slice(split + 2) };
}

/** `ui://` resource of each `server/tool`, once the server's tools are known. */
const appResources = signal<Record<string, string | null>>({});
const requestedServers = new Set<string>();

function uiResourceUri(def: unknown): string | null {
	if (!(isRecord(def) && isRecord(def._meta))) return null;
	const meta = def._meta;
	const nested = isRecord(meta.ui) ? str(meta.ui.resourceUri) : "";
	const uri = nested || str(meta["ui/resourceUri"]);
	return uri.startsWith("ui://") ? uri : null;
}

async function loadServerTools(server: string): Promise<void> {
	const res = await sendRpc<unknown[]>("mcp.tools", { name: server });
	const defs = res.ok && Array.isArray(res.payload) ? res.payload : [];
	const next = { ...appResources.value };
	for (const def of defs) {
		if (isRecord(def) && typeof def.name === "string") next[`${server}/${def.name}`] = uiResourceUri(def);
	}
	appResources.value = next;
}

/** The view URI of an MCP tool call, or null; starts the lookup on first use. */
export function mcpAppUri(item: ToolItem): string | null {
	const parts = mcpToolParts(item.name);
	if (!parts) return null;
	if (!requestedServers.has(parts.server)) {
		requestedServers.add(parts.server);
		void loadServerTools(parts.server);
	}
	return appResources.value[`${parts.server}/${parts.tool}`] ?? null;
}

interface UiResource {
	html: string;
	uiMeta?: { csp?: { connectDomains?: unknown; resourceDomains?: unknown } };
}

function stringList(value: unknown): string[] {
	return Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === "string") : [];
}

function declaredNetwork(resource: UiResource): FrameNetwork {
	const csp = resource.uiMeta?.csp;
	return { connectDomains: stringList(csp?.connectDomains), resourceDomains: stringList(csp?.resourceDomains) };
}

function currentTheme(): "light" | "dark" {
	return document.documentElement.getAttribute("data-theme") === "dark" ? "dark" : "light";
}

interface McpAppViewProps {
	item: ToolItem;
	uri: string;
}

export function McpAppView({ item, uri }: McpAppViewProps) {
	const { t } = useTranslation("canvas");
	const parts = mcpToolParts(item.name);
	const [url, setUrl] = useState<string | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [height, setHeight] = useState(240);
	const frameRef = useRef<HTMLIFrameElement | null>(null);

	useEffect(() => {
		if (!parts) return;
		let cancelled = false;
		void (async () => {
			const res = await sendRpc<UiResource>("mcp.apps.resource", { server: parts.server, uri });
			if (cancelled) return;
			if (!(res.ok && res.payload?.html)) {
				setError(res.error?.message || "no document");
				return;
			}
			try {
				const link = await stageFrame("mcp-app", res.payload.html, declaredNetwork(res.payload));
				if (!cancelled) setUrl(link.url);
			} catch (stageError) {
				if (!cancelled) setError(stageError instanceof Error ? stageError.message : String(stageError));
			}
		})();
		return () => {
			cancelled = true;
		};
	}, [parts?.server, uri]);

	const onFrame = useCallback((frame: HTMLIFrameElement | null) => {
		frameRef.current = frame;
	}, []);

	if (!parts) return null;
	if (error) return <div className="an-hint is-error">{t("appUnavailable", { error })}</div>;
	if (!url) return <div className="an-hint">{t("appLoading")}</div>;

	const context: HostContext = {
		theme: currentTheme(),
		locale: locale.value,
		toolName: parts.tool,
		arguments: Object.fromEntries(Object.entries(item.args).filter(([key]) => !key.startsWith("_"))),
		result: item.result,
	};
	const onMessage = (event: MessageEvent) => {
		const message = parseJsonRpc(event.data);
		const target = frameRef.current?.contentWindow;
		if (!(message && target)) return;
		const action = handleViewMessage(message, context);
		// Opaque-origin views can only be reached with "*"; the frame identity
		// was checked on the incoming message.
		if (action.kind === "reply") target.postMessage(action.message, "*");
		else if (action.kind === "send-tool-data") for (const out of action.messages) target.postMessage(out, "*");
		else if (action.kind === "resize") setHeight(action.height);
	};
	return (
		<SandboxFrame
			src={url}
			title={t("appTitle", { tool: parts.tool })}
			className="inline-frame"
			style={{ height: `${height}px` }}
			onMessage={onMessage}
			onFrame={onFrame}
			lazy={true}
			testId="mcp-app-frame"
		/>
	);
}

/** Button on design tool cards that shows the design in the pane. */
export function OpenDesignButton({ designId }: { designId: string }) {
	const { t } = useTranslation("canvas");
	return (
		<button type="button" className="an-btn an-btn--sm self-start" onClick={() => void openDesign(designId)}>
			<span className="an-i an-i-pane an-i--sm" aria-hidden="true" />
			{t("openDesign")}
		</button>
	);
}
