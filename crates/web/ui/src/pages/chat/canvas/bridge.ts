// ── Host side of the preview bridge (protocol "antre.canvas/1") ───
//
// Frames are sandboxed without allow-same-origin, so `event.origin` is the
// string "null": identity comes from `event.source`. Every field is
// validated and capped here; callers render them as text only.

import type { PickedElement } from "./types";

export const BRIDGE_PROTOCOL = "antre.canvas/1";

const CAPS = { text: 500, html: 300, selector: 500, message: 1000, title: 200, mid: 120, source: 200 };
const MAX_MIDS = 500;

export interface PinMarker {
	id: string;
	n: number;
	mid?: string;
	selector?: string;
	x?: number;
	y?: number;
}

export type FrameMessage =
	| { type: "ready"; title: string; mids: string[] }
	| { type: "pick.result"; element: PickedElement }
	| { type: "pick.cancel" }
	| { type: "resize"; height: number }
	| { type: "error"; message: string; source?: string; line?: number }
	| { type: "pin.open"; id: string };

export type HostMessage =
	| { type: "pick.start" }
	| { type: "pick.stop" }
	| { type: "highlight"; mid?: string; selector?: string }
	| { type: "pins"; pins: PinMarker[] };

type Fields = Record<string, unknown>;

function isFields(value: unknown): value is Fields {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function text(value: unknown, max: number): string | null {
	if (typeof value !== "string") return null;
	return value.length > max ? value.slice(0, max) : value;
}

function finite(value: unknown): number | null {
	return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function parseElement(value: unknown): PickedElement | null {
	if (!isFields(value)) return null;
	const selector = text(value.selector, CAPS.selector);
	const tag = text(value.tag, 32);
	const content = text(value.text, CAPS.text);
	const html = text(value.html, CAPS.html);
	const rect = isFields(value.rect) ? value.rect : null;
	if (selector === null || tag === null || content === null || html === null || !rect) return null;
	const x = finite(rect.x);
	const y = finite(rect.y);
	const width = finite(rect.width);
	const height = finite(rect.height);
	if (x === null || y === null || width === null || height === null) return null;
	const mid = text(value.mid, CAPS.mid);
	return { ...(mid ? { mid } : {}), selector, tag, text: content, html, rect: { x, y, width, height } };
}

/** A validated message from a frame, or `null` for anything else. */
export function parseFrameMessage(data: unknown): FrameMessage | null {
	if (!isFields(data) || data.protocol !== BRIDGE_PROTOCOL) return null;
	switch (data.type) {
		case "ready": {
			const mids = Array.isArray(data.mids)
				? data.mids
						.slice(0, MAX_MIDS)
						.map((mid) => text(mid, CAPS.mid))
						.filter((mid): mid is string => !!mid)
				: [];
			return { type: "ready", title: text(data.title, CAPS.title) ?? "", mids };
		}
		case "pick.result": {
			const element = parseElement(data.element);
			return element ? { type: "pick.result", element } : null;
		}
		case "pick.cancel":
			return { type: "pick.cancel" };
		case "resize": {
			const height = finite(data.height);
			return height !== null && height >= 0 ? { type: "resize", height: Math.min(height, 100_000) } : null;
		}
		case "error": {
			const message = text(data.message, CAPS.message);
			if (message === null) return null;
			const source = text(data.source, CAPS.source);
			const line = finite(data.line);
			return {
				type: "error",
				message,
				...(source ? { source } : {}),
				...(line === null ? {} : { line }),
			};
		}
		case "pin.open": {
			const id = text(data.id, 64);
			return id ? { type: "pin.open", id } : null;
		}
		default:
			return null;
	}
}

/** Whether a `message` event comes from `frame` (the only check that matters). */
export function fromFrame(event: MessageEvent, frame: HTMLIFrameElement | null): boolean {
	return !!frame && event.source === frame.contentWindow && event.origin === "null";
}

export function postToFrame(frame: HTMLIFrameElement | null, message: HostMessage): void {
	// The sandboxed document's origin is opaque, so "*" is the only target
	// that reaches it; nothing secret is ever sent.
	frame?.contentWindow?.postMessage({ protocol: BRIDGE_PROTOCOL, ...message }, "*");
}

/** Preview URL with the host origin the bridge must answer to. */
export function withHostHash(url: string): string {
	const base = url.split("#")[0];
	return `${base}#host=${encodeURIComponent(location.origin)}`;
}
