-- The table was never populated by current code; restoring it keeps the
-- migration reversible in shape without recreating live data.
CREATE TABLE t_device (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    platform TEXT NOT NULL,
    is_local BOOL NOT NULL,
    created_at BIGINT NOT NULL
);
