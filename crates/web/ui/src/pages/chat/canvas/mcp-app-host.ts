// ── Host side of MCP Apps (`ui://` views) ───────────────────
//
// The view talks JSON-RPC 2.0 over postMessage. This host answers
// `ui/initialize`, then sends the tool input and result once the view says
// it is initialized, and follows its size changes. Requests that would act
// for the user (calling tools, opening links, posting messages) are refused
// for now; see docs/src/canvas.md.

export const MCP_APPS_PROTOCOL_VERSION = "2026-01-26";
const METHOD_NOT_FOUND = -32601;
const MIN_HEIGHT = 60;
const MAX_HEIGHT = 2000;

export interface JsonRpcMessage {
	jsonrpc: "2.0";
	id?: string | number;
	method?: string;
	params?: Record<string, unknown>;
}

export interface HostContext {
	theme: "light" | "dark";
	locale: string;
	toolName: string;
	arguments: Record<string, unknown>;
	result: unknown;
}

export type HostAction =
	| { kind: "reply"; message: Record<string, unknown> }
	| { kind: "send-tool-data"; messages: Record<string, unknown>[] }
	| { kind: "resize"; height: number }
	| { kind: "ignore" };

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function parseJsonRpc(data: unknown): JsonRpcMessage | null {
	if (!isRecord(data) || data.jsonrpc !== "2.0") return null;
	const id = typeof data.id === "string" || typeof data.id === "number" ? data.id : undefined;
	const method = typeof data.method === "string" ? data.method.slice(0, 200) : undefined;
	const params = isRecord(data.params) ? data.params : undefined;
	return { jsonrpc: "2.0", id, method, params };
}

/** The result as the server returned it, without the untrusted-server wrapper. */
export function unwrapToolResult(result: unknown): unknown {
	if (isRecord(result) && typeof result.source === "string" && "result" in result && "note" in result) {
		return result.result;
	}
	return result;
}

/** A `CallToolResult` rebuilt from the flattened result the gateway keeps. */
export function callToolResult(result: unknown): Record<string, unknown> {
	const value = unwrapToolResult(result);
	const text = typeof value === "string" ? value : JSON.stringify(value ?? null);
	return {
		content: [{ type: "text", text }],
		...(isRecord(value) ? { structuredContent: value } : {}),
	};
}

function toolDataMessages(context: HostContext): Record<string, unknown>[] {
	return [
		{ jsonrpc: "2.0", method: "ui/notifications/tool-input", params: { arguments: context.arguments } },
		{ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: callToolResult(context.result) },
	];
}

/** What to do with one message from the view. */
export function handleViewMessage(message: JsonRpcMessage, context: HostContext): HostAction {
	const { id, method, params } = message;
	if (method === "ui/initialize" && id !== undefined) {
		return {
			kind: "reply",
			message: {
				jsonrpc: "2.0",
				id,
				result: {
					protocolVersion: MCP_APPS_PROTOCOL_VERSION,
					hostInfo: { name: "moltis", version: "1" },
					hostCapabilities: {},
					hostContext: {
						theme: context.theme,
						locale: context.locale,
						displayMode: "inline",
						availableDisplayModes: ["inline"],
						toolInfo: { tool: { name: context.toolName } },
					},
				},
			},
		};
	}
	if (method === "ui/notifications/initialized") {
		return { kind: "send-tool-data", messages: toolDataMessages(context) };
	}
	if (method === "ui/notifications/size-changed") {
		const height = typeof params?.height === "number" && Number.isFinite(params.height) ? params.height : null;
		if (height === null) return { kind: "ignore" };
		return { kind: "resize", height: Math.round(Math.min(Math.max(height, MIN_HEIGHT), MAX_HEIGHT)) };
	}
	if (id !== undefined && method) {
		return {
			kind: "reply",
			message: {
				jsonrpc: "2.0",
				id,
				error: { code: METHOD_NOT_FOUND, message: `${method} is not supported by this host` },
			},
		};
	}
	return { kind: "ignore" };
}
