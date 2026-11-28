// ── Global window augmentations ─────────────────────────────
//
// Ambient declarations for custom properties attached to `window`
// across the antre web UI. This file has no imports/exports so
// its declarations are visible to every compilation unit, including
// standalone entry points like login-app.tsx.

interface AntreStores {
	sessionStore: typeof import("../stores/session-store").sessionStore;
	modelStore: typeof import("../stores/model-store");
	projectStore: typeof import("../stores/project-store");
}

interface Window {
	/** Server-injected data (gon pattern). See gon.ts for typed access. */
	__ANTRE__?: Partial<import("./gon").GonData>;
	/** Suppress the next password-changed WebSocket redirect. */
	__antreSuppressNextPasswordChangedRedirect?: boolean;
	/** Exposed stores for E2E test access. */
	__antre_stores?: AntreStores;
	/** Exposed state module for E2E test WS connection checks. */
	__antre_state?: Record<string, unknown>;
	/** Exposed bundled modules for E2E test dynamic imports. */
	__antre_modules?: Record<string, Record<string, unknown>>;
}
