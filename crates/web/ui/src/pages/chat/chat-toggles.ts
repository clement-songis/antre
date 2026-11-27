// ── Per-session chat toggles shown in the session menu ──────

import { signal } from "@preact/signals";

/** Whether MCP tools are enabled for the session on screen. */
export const mcpToolsEnabled = signal<boolean>(true);
