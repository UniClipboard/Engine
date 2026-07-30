CREATE TABLE legacy_space_bootstrap_log (
    bootstrap_id TEXT PRIMARY KEY NOT NULL,
    space_lookup_token TEXT NOT NULL,
    previous_epoch BIGINT NOT NULL CHECK (previous_epoch = 0),
    next_epoch BIGINT NOT NULL CHECK (next_epoch = 1),
    status TEXT NOT NULL CHECK (
        status IN ('prepared', 'staged', 'awaiting_readmission', 'complete', 'recovery_required')
    ),
    encrypted_record BLOB NOT NULL,
    encrypted_stage BLOB,
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL
);

CREATE INDEX idx_legacy_space_bootstrap_log_space_status
    ON legacy_space_bootstrap_log (space_lookup_token, status);
