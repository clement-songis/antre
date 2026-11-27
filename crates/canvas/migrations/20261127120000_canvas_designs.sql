-- Design artifacts for the canvas: one row per design directory under
-- data_dir/designs/<id>/, plus an append-only list of versions. Each version
-- points at a checkpoint snapshot of the whole directory.
CREATE TABLE IF NOT EXISTS canvas_designs (
    id              TEXT    PRIMARY KEY,
    title           TEXT    NOT NULL,
    session_key     TEXT,
    project_id      TEXT,
    design_system   TEXT,
    current_version INTEGER NOT NULL DEFAULT 0,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_canvas_designs_session
    ON canvas_designs (session_key, updated_at DESC);

CREATE INDEX IF NOT EXISTS idx_canvas_designs_updated
    ON canvas_designs (updated_at DESC);

CREATE TABLE IF NOT EXISTS canvas_design_versions (
    design_id     TEXT    NOT NULL REFERENCES canvas_designs (id) ON DELETE CASCADE,
    version       INTEGER NOT NULL,
    checkpoint_id TEXT    NOT NULL,
    content_hash  TEXT    NOT NULL,
    reason        TEXT    NOT NULL,
    session_key   TEXT,
    run_id        TEXT,
    created_at    INTEGER NOT NULL,
    PRIMARY KEY (design_id, version)
);

CREATE TRIGGER IF NOT EXISTS canvas_design_versions_append_only
BEFORE UPDATE ON canvas_design_versions
BEGIN
    SELECT RAISE(ABORT, 'canvas design versions are append-only');
END;
