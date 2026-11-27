// ── Model types ─────────────────────────────────────��──────────
//
// Mirrors the JSON shape produced by `LiveModelService::list()` in
// `crates/chat/src/models.rs`. The JSON is built manually with
// camelCase keys (not derived from serde).

/** Catalog details attached to each model (`ModelCatalogDetails` in Rust). */
export interface ModelCatalogDetails {
	/** Id sent to the API, without the provider namespace. */
	wireId: string;
	/** Date or snapshot suffix split off the id. */
	versionLabel: string | null;
	/** Context window from a real source; null when unknown. */
	contextWindow: number | null;
	maxOutput: number | null;
	deprecated: boolean;
	/** `YYYY-MM-DD`. */
	retiresAt: string | null;
	/** `live+static` providers: static models no longer served, served models without an entry. */
	providerMismatch?: { missing: string[]; unknown: string[] } | null;
}

/**
 * Model info as returned by the `models.list` and `models.list_all` RPCs.
 */
export interface ModelInfo {
	id: string;
	provider: string;
	displayName: string;
	supportsTools: boolean;
	supportsVision: boolean;
	supportsReasoning: boolean;
	preferred: boolean;
	recommended: boolean;
	createdAt?: number | null;
	catalog?: ModelCatalogDetails;
	/** Present only in `models.list_all` responses. */
	disabled?: boolean;
	/** Present in `models.list` (always false) and `models.list_all`. */
	unsupported: boolean;
	unsupportedReason?: string | null;
	unsupportedProvider?: string | null;
	unsupportedUpdatedAt?: number | null;
}

/** Parsed reasoning-suffix result. */
export interface ReasoningSuffix {
	baseId: string;
	effort: string;
}
