-- Comment pins on a design version, anchored to an element (data-mid or CSS
-- selector) or to page coordinates. "Send to agent" resolves them.
CREATE TABLE IF NOT EXISTS canvas_design_comments (
    id           TEXT    PRIMARY KEY,
    design_id    TEXT    NOT NULL REFERENCES canvas_designs (id) ON DELETE CASCADE,
    version      INTEGER NOT NULL,
    mid          TEXT,
    selector     TEXT,
    element_text TEXT,
    x            INTEGER,
    y            INTEGER,
    body         TEXT    NOT NULL,
    resolved     INTEGER NOT NULL DEFAULT 0,
    created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_canvas_design_comments_design
    ON canvas_design_comments (design_id, version, created_at);
