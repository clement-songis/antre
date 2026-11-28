// ── Typed wrappers around the `canvas.*` RPC namespace ───────────

import { sendRpc } from "../../../helpers";
import type {
	Design,
	DesignComment,
	DesignVersion,
	ExportFormat,
	FrameKind,
	FrameNetwork,
	PickedElement,
	PreviewLink,
	PublishedLink,
} from "./types";

export class CanvasError extends Error {}

async function call<T>(method: string, params: Record<string, unknown>): Promise<T> {
	const res = await sendRpc<T>(method, params);
	if (!(res.ok && res.payload !== undefined)) {
		throw new CanvasError(res.error?.message || `${method} failed`);
	}
	return res.payload;
}

/** The web UI's host, so preview links reuse the host name the user typed. */
function host(): string {
	return location.host;
}

export async function listDesigns(sessionKey: string): Promise<Design[]> {
	const payload = await call<{ designs: Design[] }>("canvas.list", { sessionKey, limit: 50 });
	return payload.designs;
}

export async function getDesign(designId: string): Promise<{ design: Design; versions: DesignVersion[] }> {
	return call("canvas.get", { designId });
}

export async function listVersions(designId: string): Promise<DesignVersion[]> {
	const payload = await call<{ versions: DesignVersion[] }>("canvas.versions", { designId, limit: 200 });
	return payload.versions;
}

export function previewUrl(designId: string, version: number | null): Promise<PreviewLink> {
	return call("canvas.preview_url", { designId, version: version ?? undefined, host: host() });
}

export async function revertTo(designId: string, version: number): Promise<DesignVersion> {
	const payload = await call<{ version: DesignVersion }>("canvas.revert", { designId, version });
	return payload.version;
}

export function publishDesign(
	designId: string,
	version: number | null,
	visibility: "public" | "private",
): Promise<PublishedLink> {
	return call("canvas.publish", { designId, version: version ?? undefined, visibility, host: host() });
}

export function stageFrame(kind: FrameKind, html: string, network?: FrameNetwork): Promise<PreviewLink> {
	return call("canvas.stage", { kind, html, network: network ?? {}, host: host() });
}

export async function listComments(designId: string, version: number | null): Promise<DesignComment[]> {
	const payload = await call<{ comments: DesignComment[] }>("canvas.comments.list", {
		designId,
		version: version ?? undefined,
	});
	return payload.comments;
}

export async function addComment(
	designId: string,
	version: number | null,
	element: PickedElement,
	body: string,
): Promise<DesignComment> {
	const payload = await call<{ comment: DesignComment }>("canvas.comments.add", {
		designId,
		version: version ?? undefined,
		mid: element.mid,
		selector: element.selector,
		elementText: element.text,
		x: element.rect.x,
		y: element.rect.y,
		body,
	});
	return payload.comment;
}

export async function resolveComments(designId: string, commentIds: string[]): Promise<number> {
	const payload = await call<{ resolved: number }>("canvas.comments.resolve", { designId, commentIds });
	return payload.resolved;
}

export async function deleteComment(designId: string, commentId: string): Promise<boolean> {
	const payload = await call<{ deleted: boolean }>("canvas.comments.delete", { designId, commentId });
	return payload.deleted;
}

/** REST download URL of an export (same-origin, cookie-authenticated). */
export function exportUrl(designId: string, format: ExportFormat, version: number | null): string {
	const query = version ? `?version=${version}` : "";
	return `/api/canvas/designs/${encodeURIComponent(designId)}/export/${format}${query}`;
}

export async function createDesign(title: string, sessionKey: string, html?: string): Promise<Design> {
	const payload = await call<{ design: Design }>("canvas.create", { title, sessionKey, html });
	return payload.design;
}
