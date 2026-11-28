// ── Design canvas types (mirror `moltis-canvas` serde shapes) ─────

export interface Design {
	id: string;
	title: string;
	sessionKey?: string | null;
	projectId?: string | null;
	designSystem?: string | null;
	currentVersion: number;
	createdAt: number;
	updatedAt: number;
}

export interface DesignVersion {
	designId: string;
	version: number;
	contentHash: string;
	reason: string;
	sessionKey?: string | null;
	runId?: string | null;
	createdAt: number;
}

export interface PreviewLink {
	url: string;
	origin: string;
	expiresAt: number;
	sandbox: string;
	version?: number | null;
}

export interface DesignComment {
	id: string;
	designId: string;
	version: number;
	mid?: string;
	selector?: string;
	elementText?: string;
	x?: number;
	y?: number;
	body: string;
	resolved: boolean;
	createdAt: number;
}

export interface PublishedLink {
	shareId: string;
	url: string;
	accessKey?: string | null;
	version: number;
	visibility: "public" | "private";
}

export type ExportFormat = "html" | "zip" | "png" | "pdf";

export type FrameKind = "artifact" | "mcp-app";

export interface FrameNetwork {
	connectDomains?: string[];
	resourceDomains?: string[];
}

/** An element reported by the preview bridge (`pick.result`). */
export interface PickedElement {
	mid?: string;
	selector: string;
	tag: string;
	text: string;
	html: string;
	rect: { x: number; y: number; width: number; height: number };
}

/** A picked element attached to the composer as context for the agent. */
export interface ElementContext {
	id: string;
	designId: string;
	designTitle: string;
	version: number | null;
	element: PickedElement;
}

export type Viewport = "mobile" | "tablet" | "desktop";

export const VIEWPORT_WIDTHS: Record<Viewport, number | null> = {
	mobile: 390,
	tablet: 768,
	desktop: null,
};

// ── WebSocket event payloads ─────────────────────────────────

export interface DesignCreatedEvent {
	design: Design;
}

export interface VersionCreatedEvent {
	designId: string;
	version: DesignVersion;
	revertedFrom?: number;
}

export interface CommentsChangedEvent {
	designId: string;
	commentsVersion?: number;
}

export interface PublishedEvent {
	designId: string;
	shareId: string;
	version: number;
	visibility: "public" | "private";
}
