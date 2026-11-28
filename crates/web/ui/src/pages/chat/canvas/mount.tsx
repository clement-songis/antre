// ── Mounting the canvas pane, its header toggle and composer chips ──

import { effect } from "@preact/signals";
import { render } from "preact";
import { connected } from "../../../signals";
import * as S from "../../../state";
import { activeSessionKey } from "../../../stores/session-store";
import { CanvasPane, CanvasToggle } from "./CanvasPane";
import { ElementChips } from "./ElementChips";
import { initCanvasEvents, loadSessionDesigns, resetCanvasPane } from "./store";

const MOUNTS = ["canvasPaneMount", "canvasToggleMount", "canvasChipsMount"] as const;

let stopSessionWatch: (() => void) | null = null;
let stopEvents: (() => void) | null = null;

export function mountCanvas(): void {
	const pane = S.$("canvasPaneMount");
	const toggle = S.$("canvasToggleMount");
	const chips = S.$("canvasChipsMount");
	if (pane) render(<CanvasPane />, pane);
	if (toggle) render(<CanvasToggle />, toggle);
	if (chips) render(<ElementChips />, chips);
	stopEvents = initCanvasEvents();
	stopSessionWatch = effect(() => {
		const key = activeSessionKey.value;
		if (connected.value) void loadSessionDesigns(key);
	});
}

export function unmountCanvas(): void {
	stopSessionWatch?.();
	stopSessionWatch = null;
	stopEvents?.();
	stopEvents = null;
	for (const id of MOUNTS) {
		const mount = S.$(id);
		if (mount) render(null, mount);
	}
	resetCanvasPane();
}
