// ── Design preview pane next to the chat ────────────────────

import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { MenuButton, type MenuItem } from "../../../components/ContextMenu";
import { useTranslation } from "../../../i18n";
import { copyToClipboard } from "../../../ui";
import { exportUrl, publishDesign } from "./api";
import { fromFrame, type PinMarker, parseFrameMessage, postToFrame, withHostHash } from "./bridge";
import { CommentsPanel } from "./CommentsPanel";
import { SandboxFrame } from "./SandboxFrame";
import {
	activeDesign,
	addElementContext,
	closePane,
	comments,
	designs,
	fullscreen,
	MIN_PANE_WIDTH,
	mode,
	openDesign,
	paneError,
	paneOpen,
	paneWidth,
	pendingComment,
	previewLink,
	recordFrameError,
	reloadNonce,
	revertToVersion,
	selectVersion,
	setMode,
	setPaneWidth,
	shownVersion,
	versions,
	viewport,
} from "./store";
import type { ExportFormat, PublishedLink, Viewport } from "./types";
import { VIEWPORT_WIDTHS } from "./types";

const VIEWPORTS: { id: Viewport; icon: string }[] = [
	{ id: "mobile", icon: "an-i-phone" },
	{ id: "tablet", icon: "an-i-frame" },
	{ id: "desktop", icon: "an-i-sidebar" },
];

const EXPORTS: ExportFormat[] = ["html", "zip", "png", "pdf"];

function download(url: string): void {
	const link = document.createElement("a");
	link.href = url;
	link.download = "";
	link.rel = "noopener";
	document.body.appendChild(link);
	link.click();
	link.remove();
}

function focusComposer(): void {
	(document.getElementById("chatInput") as HTMLTextAreaElement | null)?.focus();
}

function ResizeHandle() {
	const { t } = useTranslation("canvas");
	const onPointerDown = (event: PointerEvent) => {
		event.preventDefault();
		const handle = event.currentTarget as HTMLElement;
		handle.setPointerCapture(event.pointerId);
		const startX = event.clientX;
		const startWidth = paneWidth.value;
		const move = (moveEvent: PointerEvent) => setPaneWidth(startWidth + (startX - moveEvent.clientX));
		const up = () => {
			handle.removeEventListener("pointermove", move);
			handle.removeEventListener("pointerup", up);
			handle.removeEventListener("pointercancel", up);
		};
		handle.addEventListener("pointermove", move);
		handle.addEventListener("pointerup", up);
		handle.addEventListener("pointercancel", up);
	};
	return (
		// biome-ignore lint/a11y/useSemanticElements: a draggable separator has no native element.
		<div
			role="separator"
			aria-orientation="vertical"
			aria-label={t("resize")}
			aria-valuemin={MIN_PANE_WIDTH}
			aria-valuenow={paneWidth.value}
			tabIndex={0}
			className="canvas-pane-handle"
			onPointerDown={onPointerDown}
			onKeyDown={(event) => {
				if (event.key === "ArrowLeft") setPaneWidth(paneWidth.value + 24);
				else if (event.key === "ArrowRight") setPaneWidth(paneWidth.value - 24);
				else return;
				event.preventDefault();
			}}
		/>
	);
}

function PublishResult({ published, onClose }: { published: PublishedLink; onClose: () => void }) {
	const { t } = useTranslation("canvas");
	return (
		<div className="an-banner flex-col items-stretch gap-2" role="status" data-testid="canvas-published">
			<div className="flex items-center gap-2">
				<span className="an-i an-i-link an-i--sm" aria-hidden="true" />
				<span className="an-strong flex-1">
					{t(published.visibility === "public" ? "publishedPublic" : "publishedPrivate", {
						version: published.version,
					})}
				</span>
				<button
					type="button"
					className="an-btn an-btn--ghost an-btn--sm an-btn--icon"
					aria-label={t("dismiss")}
					onClick={onClose}
				>
					<span className="an-i an-i-x an-i--sm" aria-hidden="true" />
				</button>
			</div>
			<div className="flex items-center gap-2">
				<input
					className="an-input an-input--mono h-8 flex-1 text-xs"
					readOnly
					value={published.url}
					aria-label={t("publishedUrl")}
				/>
				<button type="button" className="an-btn an-btn--sm" onClick={() => void copyToClipboard(published.url)}>
					<span className="an-i an-i-copy an-i--sm" aria-hidden="true" />
					{t("copy")}
				</button>
			</div>
			{published.accessKey ? <span className="an-hint">{t("accessKeyNote")}</span> : null}
		</div>
	);
}

function VersionPicker() {
	const { t } = useTranslation("canvas");
	const design = activeDesign.value;
	const shown = shownVersion.value;
	if (!design) return null;
	return (
		<div className="flex items-center gap-1">
			<select
				className="an-input an-select h-7 w-auto py-0 text-xs"
				aria-label={t("version")}
				value={shown === null ? "live" : String(shown)}
				onChange={(event) => {
					const value = (event.currentTarget as HTMLSelectElement).value;
					void selectVersion(value === "live" ? null : Number(value));
				}}
			>
				<option value="live">{t("liveVersion", { version: design.currentVersion })}</option>
				{versions.value.map((version) => (
					<option key={version.version} value={String(version.version)}>
						{t("versionOption", { version: version.version, reason: version.reason })}
					</option>
				))}
			</select>
			{shown !== null && shown !== design.currentVersion ? (
				<button type="button" className="an-btn an-btn--sm" onClick={() => void revertToVersion(shown)}>
					<span className="an-i an-i-rewind an-i--sm" aria-hidden="true" />
					{t("revertTo", { version: shown })}
				</button>
			) : null}
		</div>
	);
}

export function CanvasPane() {
	const { t } = useTranslation("canvas");
	const frameRef = useRef<HTMLIFrameElement | null>(null);
	const [ready, setReady] = useState(false);
	const [focusedComment, setFocusedComment] = useState<string | null>(null);
	const [published, setPublished] = useState<PublishedLink | null>(null);
	const [busy, setBusy] = useState(false);
	const design = activeDesign.value;
	const link = previewLink.value;
	const currentMode = mode.value;

	const onFrame = useCallback((frame: HTMLIFrameElement | null) => {
		frameRef.current = frame;
	}, []);

	useEffect(() => {
		if (!ready) return;
		postToFrame(frameRef.current, { type: currentMode === "view" ? "pick.stop" : "pick.start" });
	}, [currentMode, ready]);

	const pins: PinMarker[] = comments.value.map((comment, index) => ({
		id: comment.id,
		n: index + 1,
		mid: comment.mid,
		selector: comment.selector,
		x: comment.x,
		y: comment.y,
	}));
	const pinsKey = JSON.stringify(pins);
	useEffect(() => {
		if (ready) postToFrame(frameRef.current, { type: "pins", pins });
		// biome-ignore lint/correctness/useExhaustiveDependencies: pinsKey stands for pins.
	}, [pinsKey, ready]);

	useEffect(() => {
		setReady(false);
	}, [link?.url, reloadNonce.value]);

	useEffect(() => {
		setPublished(null);
	}, [design?.id]);

	useEffect(() => {
		const onKey = (event: KeyboardEvent) => {
			if (event.key !== "Escape" || !fullscreen.value || mode.value !== "view" || pendingComment.value) return;
			event.preventDefault();
			fullscreen.value = false;
		};
		document.addEventListener("keydown", onKey, true);
		return () => document.removeEventListener("keydown", onKey, true);
	}, []);

	if (!paneOpen.value) return null;

	const onMessage = (event: MessageEvent) => {
		if (!fromFrame(event, frameRef.current)) return;
		const message = parseFrameMessage(event.data);
		if (!message) return;
		switch (message.type) {
			case "ready":
				setReady(true);
				break;
			case "pick.result":
				if (mode.value === "pick") {
					addElementContext(message.element);
					mode.value = "view";
					focusComposer();
				} else if (mode.value === "comment") {
					pendingComment.value = message.element;
					mode.value = "view";
				}
				break;
			case "pick.cancel":
				mode.value = "view";
				break;
			case "error":
				recordFrameError(message);
				break;
			case "pin.open":
				setFocusedComment(message.id);
				break;
			default:
				break;
		}
	};

	const publish = async (visibility: "public" | "private") => {
		if (!design) return;
		setBusy(true);
		try {
			setPublished(await publishDesign(design.id, shownVersion.value, visibility));
		} catch (error) {
			paneError.value = error instanceof Error ? error.message : String(error);
		} finally {
			setBusy(false);
		}
	};

	const exportMenu = (): { label: string; items: MenuItem[] } | null =>
		design
			? {
					label: t("export"),
					items: EXPORTS.map((format) => ({
						id: `export-${format}`,
						label: t(`export_${format}`),
						icon: "icon-document",
						onSelect: () => download(exportUrl(design.id, format, shownVersion.value)),
					})),
				}
			: null;

	const publishMenu = (): { label: string; items: MenuItem[] } | null =>
		design
			? {
					label: t("publish"),
					items: [
						{
							id: "publish-private",
							label: t("publishPrivate"),
							icon: "icon-lock",
							onSelect: () => void publish("private"),
						},
						{
							id: "publish-public",
							label: t("publishPublic"),
							icon: "icon-globe",
							onSelect: () => void publish("public"),
						},
					],
				}
			: null;

	const designMenu = (): { label: string; items: MenuItem[] } | null =>
		designs.value.length > 1
			? {
					label: t("designs"),
					items: designs.value.map((entry) => ({
						id: `design-${entry.id}`,
						label: entry.title,
						onSelect: () => void openDesign(entry.id),
					})),
				}
			: null;

	const width = VIEWPORT_WIDTHS[viewport.value];
	const isFullscreen = fullscreen.value;

	return (
		<aside
			className={`an-pane canvas-pane ${isFullscreen ? "canvas-pane--fullscreen" : ""}`}
			style={isFullscreen ? undefined : { width: `${paneWidth.value}px`, flexBasis: `${paneWidth.value}px` }}
			aria-label={t("paneLabel")}
			data-testid="canvas-pane"
		>
			{isFullscreen ? null : <ResizeHandle />}
			<div className="an-pane__tabs">
				<span className="an-pane__tab is-active min-w-0">
					<span className="an-i an-i-palette an-i--sm" aria-hidden="true" />
					<span className="truncate" data-testid="canvas-title">
						{design?.title ?? t("loading")}
					</span>
				</span>
				{designs.value.length > 1 ? (
					<MenuButton
						getMenu={designMenu}
						label={t("switchDesign")}
						className="an-btn an-btn--ghost an-btn--sm an-btn--icon"
					>
						<span className="an-i an-i-chevron-down an-i--sm" aria-hidden="true" />
					</MenuButton>
				) : null}
				<span className="an-grow" />
				<button
					type="button"
					className="an-btn an-btn--ghost an-btn--sm an-btn--icon"
					aria-label={t(isFullscreen ? "exitFullscreen" : "fullscreen")}
					title={t(isFullscreen ? "exitFullscreen" : "fullscreen")}
					aria-pressed={isFullscreen}
					onClick={() => (fullscreen.value = !isFullscreen)}
				>
					<span className={`an-i an-i--sm ${isFullscreen ? "an-i-shrink" : "an-i-expand"}`} aria-hidden="true" />
				</button>
				<button
					type="button"
					className="an-btn an-btn--ghost an-btn--sm an-btn--icon"
					aria-label={t("close")}
					title={t("close")}
					onClick={closePane}
				>
					<span className="an-i an-i-x an-i--sm" aria-hidden="true" />
				</button>
			</div>
			<div className="flex flex-wrap items-center gap-2 border-b border-line px-2 py-2">
				<VersionPicker />
				<span className="an-grow" />
				<fieldset className="an-segment m-0 min-w-0">
					<legend className="sr-only">{t("viewport")}</legend>
					{VIEWPORTS.map((entry) => (
						<button
							key={entry.id}
							type="button"
							className={viewport.value === entry.id ? "is-active" : ""}
							aria-pressed={viewport.value === entry.id}
							aria-label={t(`viewport_${entry.id}`)}
							title={t(`viewport_${entry.id}`)}
							onClick={() => (viewport.value = entry.id)}
						>
							<span className={`an-i an-i--sm ${entry.icon}`} aria-hidden="true" />
						</button>
					))}
				</fieldset>
				<fieldset className="an-segment m-0 min-w-0">
					<legend className="sr-only">{t("tools")}</legend>
					<button
						type="button"
						className={currentMode === "pick" ? "is-active" : ""}
						aria-pressed={currentMode === "pick"}
						title={t("pickHint")}
						onClick={() => setMode("pick")}
					>
						<span className="an-i an-i-pointer an-i--sm" aria-hidden="true" />
						{t("pick")}
					</button>
					<button
						type="button"
						className={currentMode === "comment" ? "is-active" : ""}
						aria-pressed={currentMode === "comment"}
						title={t("commentHint")}
						onClick={() => setMode("comment")}
					>
						<span className="an-i an-i-chat an-i--sm" aria-hidden="true" />
						{t("comment")}
					</button>
				</fieldset>
				<MenuButton getMenu={exportMenu} label={t("export")} className="an-btn an-btn--sm">
					<span className="an-i an-i-download an-i--sm" aria-hidden="true" />
					{t("export")}
				</MenuButton>
				<MenuButton getMenu={publishMenu} label={t("publish")} className="an-btn an-btn--primary an-btn--sm">
					<span className="an-i an-i-upload an-i--sm" aria-hidden="true" />
					{busy ? t("publishing") : t("publish")}
				</MenuButton>
			</div>
			<div className="an-pane__body">
				{paneError.value ? (
					<div className="an-banner an-banner--warn" role="alert">
						<span className="an-i an-i-warn an-i--sm" aria-hidden="true" />
						<span className="flex-1">{paneError.value}</span>
						<button type="button" className="an-btn an-btn--ghost an-btn--sm" onClick={() => (paneError.value = null)}>
							{t("dismiss")}
						</button>
					</div>
				) : null}
				{published ? <PublishResult published={published} onClose={() => setPublished(null)} /> : null}
				{currentMode === "view" ? null : (
					<div className="an-hint" role="status">
						{t(currentMode === "pick" ? "pickActive" : "commentActive")}
					</div>
				)}
				<div className="canvas-stage">
					{link ? (
						<SandboxFrame
							key={`${link.url}#${reloadNonce.value}`}
							src={withHostHash(link.url)}
							title={t("frameTitle", { title: design?.title ?? "" })}
							className="canvas-frame"
							style={width ? { width: `${width}px` } : undefined}
							onMessage={onMessage}
							onFrame={onFrame}
							testId="canvas-frame"
						/>
					) : (
						<div className="an-empty">{t("loading")}</div>
					)}
				</div>
				<CommentsPanel
					focusedId={focusedComment}
					onHighlight={(target) => postToFrame(frameRef.current, { type: "highlight", ...target })}
				/>
			</div>
		</aside>
	);
}

/** Header button that opens or closes the pane; hidden without designs. */
export function CanvasToggle() {
	const { t } = useTranslation("canvas");
	if (designs.value.length === 0 && !activeDesign.value) return null;
	const open = paneOpen.value;
	return (
		<button
			type="button"
			className={`an-btn an-btn--ghost an-btn--sm an-btn--icon ${open ? "text-lake" : ""}`}
			aria-label={t("togglePane")}
			title={t("togglePane")}
			aria-pressed={open}
			data-testid="canvas-toggle"
			onClick={() => {
				if (open) closePane();
				else {
					const target = activeDesign.value ?? designs.value[0];
					if (target) void openDesign(target.id);
				}
			}}
		>
			<span className="an-i an-i-pane" aria-hidden="true" />
		</button>
	);
}
