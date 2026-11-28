// ── Design canvas pane state ────────────────────────────────
//
// One preview pane per chat page. It follows the active session: it opens
// when the agent creates or changes a design of that session (unless the
// user closed it for that design), reloads on every new version, and keeps
// element contexts picked for the composer.

import { signal } from "@preact/signals";
import { onEvent } from "../../../events";
import { activeSessionKey } from "../../../stores/session-store";
import * as api from "./api";
import type {
	CommentsChangedEvent,
	Design,
	DesignComment,
	DesignCreatedEvent,
	DesignVersion,
	ElementContext,
	PickedElement,
	PreviewLink,
	VersionCreatedEvent,
	Viewport,
} from "./types";

export type PaneMode = "view" | "pick" | "comment";

export interface FrameError {
	message: string;
	source?: string;
	line?: number;
}

const WIDTH_KEY = "moltis-canvas-pane-width";
export const MIN_PANE_WIDTH = 320;
export const DEFAULT_PANE_WIDTH = 560;
const MAX_FRAME_ERRORS = 20;
const MAX_ELEMENT_CONTEXTS = 8;
/** Refresh preview links this long before they expire. */
const LINK_REFRESH_MARGIN_MS = 60_000;

function storedWidth(): number {
	const raw = Number(localStorage.getItem(WIDTH_KEY));
	return Number.isFinite(raw) && raw >= MIN_PANE_WIDTH ? raw : DEFAULT_PANE_WIDTH;
}

export const paneOpen = signal<boolean>(false);
export const paneWidth = signal<number>(storedWidth());
export const fullscreen = signal<boolean>(false);
export const designs = signal<Design[]>([]);
export const activeDesign = signal<Design | null>(null);
export const versions = signal<DesignVersion[]>([]);
/** `null` shows the live files. */
export const shownVersion = signal<number | null>(null);
export const previewLink = signal<PreviewLink | null>(null);
/** Bumped to reload the frame without a new link. */
export const reloadNonce = signal<number>(0);
export const viewport = signal<Viewport>("desktop");
export const mode = signal<PaneMode>("view");
export const comments = signal<DesignComment[]>([]);
export const pendingComment = signal<PickedElement | null>(null);
export const frameErrors = signal<FrameError[]>([]);
export const paneError = signal<string | null>(null);
export const elementContexts = signal<ElementContext[]>([]);

/** Designs the user closed the pane on; agent updates do not reopen them. */
const dismissed = new Set<string>();
let linkTimer: ReturnType<typeof setTimeout> | null = null;
let loadToken = 0;

function report(error: unknown): void {
	paneError.value = error instanceof Error ? error.message : String(error);
}

export function setPaneWidth(width: number): void {
	const max = Math.max(MIN_PANE_WIDTH, window.innerWidth - 320);
	const clamped = Math.round(Math.min(Math.max(width, MIN_PANE_WIDTH), max));
	paneWidth.value = clamped;
	localStorage.setItem(WIDTH_KEY, String(clamped));
}

function scheduleLinkRefresh(link: PreviewLink): void {
	if (linkTimer) clearTimeout(linkTimer);
	const delay = link.expiresAt * 1000 - Date.now() - LINK_REFRESH_MARGIN_MS;
	linkTimer = setTimeout(() => void refreshLink(), Math.max(delay, 5_000));
}

export async function refreshLink(): Promise<void> {
	const design = activeDesign.value;
	if (!design) return;
	const token = loadToken;
	try {
		const link = await api.previewUrl(design.id, shownVersion.value);
		if (token !== loadToken) return;
		previewLink.value = link;
		scheduleLinkRefresh(link);
	} catch (error) {
		if (token === loadToken) report(error);
	}
}

export async function refreshComments(): Promise<void> {
	const design = activeDesign.value;
	if (!design) return;
	const version = shownVersion.value ?? design.currentVersion;
	try {
		comments.value = await api.listComments(design.id, version || null);
	} catch (error) {
		report(error);
	}
}

/** Show `designId` in the pane (live files unless `version` is given). */
export async function openDesign(designId: string, version: number | null = null): Promise<void> {
	loadToken += 1;
	const token = loadToken;
	dismissed.delete(designId);
	paneOpen.value = true;
	paneError.value = null;
	frameErrors.value = [];
	mode.value = "view";
	pendingComment.value = null;
	shownVersion.value = version;
	try {
		const details = await api.getDesign(designId);
		if (token !== loadToken) return;
		activeDesign.value = details.design;
		versions.value = details.versions;
		if (!designs.value.some((design) => design.id === designId)) {
			designs.value = [details.design, ...designs.value];
		}
		await Promise.all([refreshLink(), refreshComments()]);
	} catch (error) {
		if (token === loadToken) report(error);
	}
}

export function closePane(): void {
	const design = activeDesign.value;
	if (design) dismissed.add(design.id);
	paneOpen.value = false;
	fullscreen.value = false;
	mode.value = "view";
	pendingComment.value = null;
}

export function togglePane(): void {
	if (paneOpen.value) {
		closePane();
		return;
	}
	const target = activeDesign.value ?? designs.value[0];
	if (target) void openDesign(target.id);
}

export async function selectVersion(version: number | null): Promise<void> {
	shownVersion.value = version;
	pendingComment.value = null;
	await Promise.all([refreshLink(), refreshComments()]);
}

export async function revertToVersion(version: number): Promise<void> {
	const design = activeDesign.value;
	if (!design) return;
	try {
		await api.revertTo(design.id, version);
		await selectVersion(null);
	} catch (error) {
		report(error);
	}
}

export function setMode(next: PaneMode): void {
	mode.value = mode.value === next ? "view" : next;
	if (mode.value !== "comment") pendingComment.value = null;
}

export function recordFrameError(error: FrameError): void {
	frameErrors.value = [...frameErrors.value, error].slice(-MAX_FRAME_ERRORS);
}

export function clearFrameErrors(): void {
	frameErrors.value = [];
}

export function addElementContext(element: PickedElement): void {
	const design = activeDesign.value;
	if (!design) return;
	const context: ElementContext = {
		id: `${design.id}:${element.mid ?? element.selector}`,
		designId: design.id,
		designTitle: design.title,
		version: shownVersion.value ?? (design.currentVersion || null),
		element,
	};
	const others = elementContexts.value.filter((existing) => existing.id !== context.id);
	elementContexts.value = [...others, context].slice(-MAX_ELEMENT_CONTEXTS);
}

export function removeElementContext(id: string): void {
	elementContexts.value = elementContexts.value.filter((context) => context.id !== id);
}

/** Take (and clear) the composer's element contexts. */
export function takeElementContexts(): ElementContext[] {
	const taken = elementContexts.value;
	elementContexts.value = [];
	return taken;
}

export async function saveComment(body: string): Promise<boolean> {
	const design = activeDesign.value;
	const element = pendingComment.value;
	if (!(design && element && body.trim())) return false;
	try {
		await api.addComment(design.id, shownVersion.value, element, body.trim());
		pendingComment.value = null;
		await refreshComments();
		return true;
	} catch (error) {
		report(error);
		return false;
	}
}

export async function removeComment(commentId: string): Promise<void> {
	const design = activeDesign.value;
	if (!design) return;
	try {
		await api.deleteComment(design.id, commentId);
		await refreshComments();
	} catch (error) {
		report(error);
	}
}

/** Resolve comments once they were sent to the agent. */
export async function resolveSentComments(sent: DesignComment[]): Promise<void> {
	const design = activeDesign.value;
	if (!(design && sent.length)) return;
	try {
		await api.resolveComments(
			design.id,
			sent.map((comment) => comment.id),
		);
		await refreshComments();
	} catch (error) {
		report(error);
	}
}

/** Reload the designs of `sessionKey`; closes a pane showing another session's design. */
export async function loadSessionDesigns(sessionKey: string): Promise<void> {
	try {
		const list = await api.listDesigns(sessionKey);
		if (activeSessionKey.value !== sessionKey) return;
		designs.value = list;
		const shown = activeDesign.value;
		if (shown && !list.some((design) => design.id === shown.id)) {
			loadToken += 1;
			activeDesign.value = null;
			previewLink.value = null;
			paneOpen.value = false;
			fullscreen.value = false;
		}
	} catch {
		designs.value = [];
	}
}

function belongsToActiveSession(design: Design): boolean {
	return !design.sessionKey || design.sessionKey === activeSessionKey.value;
}

function onDesignCreated(payload: DesignCreatedEvent): void {
	const design = payload.design;
	if (!(design && belongsToActiveSession(design))) return;
	designs.value = [design, ...designs.value.filter((known) => known.id !== design.id)];
	void openDesign(design.id);
}

function onVersionCreated(payload: VersionCreatedEvent): void {
	const shown = activeDesign.value;
	if (shown?.id === payload.designId) {
		activeDesign.value = { ...shown, currentVersion: payload.version.version };
		versions.value = [payload.version, ...versions.value.filter((v) => v.version !== payload.version.version)];
		if (shownVersion.value === null) {
			reloadNonce.value += 1;
			frameErrors.value = [];
			void refreshComments();
		}
		if (!(paneOpen.value || dismissed.has(shown.id))) paneOpen.value = true;
		return;
	}
	const known = designs.value.find((design) => design.id === payload.designId);
	if (known && !dismissed.has(known.id)) void openDesign(known.id);
}

function onCommentsChanged(payload: CommentsChangedEvent): void {
	if (activeDesign.value?.id === payload.designId) void refreshComments();
}

let unsubscribe: (() => void) | null = null;

/** Subscribe to canvas events; idempotent. */
export function initCanvasEvents(): () => void {
	if (unsubscribe) return unsubscribe;
	const offs = [
		onEvent("canvas.design.created", (payload) => onDesignCreated(payload as DesignCreatedEvent)),
		onEvent("canvas.version.created", (payload) => onVersionCreated(payload as VersionCreatedEvent)),
		onEvent("canvas.comments.changed", (payload) => onCommentsChanged(payload as CommentsChangedEvent)),
	];
	unsubscribe = () => {
		for (const off of offs) off();
		unsubscribe = null;
		if (linkTimer) clearTimeout(linkTimer);
		linkTimer = null;
	};
	return unsubscribe;
}

/** Forget everything; used when the chat page unmounts. */
export function resetCanvasPane(): void {
	loadToken += 1;
	paneOpen.value = false;
	fullscreen.value = false;
	activeDesign.value = null;
	versions.value = [];
	previewLink.value = null;
	comments.value = [];
	pendingComment.value = null;
	frameErrors.value = [];
	paneError.value = null;
	mode.value = "view";
	shownVersion.value = null;
	if (linkTimer) clearTimeout(linkTimer);
	linkTimer = null;
}
