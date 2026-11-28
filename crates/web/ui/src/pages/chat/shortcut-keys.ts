// Key specs only, so the command palette can show them without pulling in the
// chat and session modules (the onboarding bundle shares the palette).

import { signal } from "@preact/signals";

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform || "");
const MOD = isMac ? "⌘" : "Ctrl";

export interface ShortcutSpec {
	id: string;
	/** Translation key under `chat:shortcuts`. */
	labelKey: string;
	keys: string[];
}

/** Every shortcut, in the order the help dialog lists them. */
export const SHORTCUTS: ShortcutSpec[] = [
	{ id: "new-chat", labelKey: "newChat", keys: [MOD, "Shift", "O"] },
	{ id: "focus", labelKey: "focusComposer", keys: ["Shift", "Esc"] },
	{ id: "sidebar", labelKey: "toggleSidebar", keys: [MOD, "Shift", "S"] },
	{ id: "next", labelKey: "nextSession", keys: ["Alt", "Shift", "↓"] },
	{ id: "prev", labelKey: "prevSession", keys: ["Alt", "Shift", "↑"] },
	{ id: "copy", labelKey: "copyLast", keys: ["Alt", "Shift", "C"] },
	{ id: "stop", labelKey: "stop", keys: ["Esc"] },
	{ id: "rewind", labelKey: "rewind", keys: ["Esc", "Esc"] },
	{ id: "palette", labelKey: "palette", keys: [MOD, "K"] },
	{ id: "help", labelKey: "help", keys: [MOD, "/"] },
];

export function shortcutText(id: string): string {
	const spec = SHORTCUTS.find((entry) => entry.id === id);
	return spec ? spec.keys.join(isMac ? "" : "+") : "";
}

export const shortcutsHelpOpen = signal(false);
