CREATE TABLE space_key_epoch_state (
    space_id TEXT PRIMARY KEY NOT NULL,
    group_epoch BIGINT NOT NULL CHECK (group_epoch >= 0),
    security_mode TEXT NOT NULL CHECK (security_mode IN ('legacy', 'migrating', 'ready')),
    current_content_key_id TEXT NOT NULL,
    encrypted_payload BLOB NOT NULL,
    updated_at_ms BIGINT NOT NULL
);

CREATE TABLE member_revocation_log (
    revocation_id TEXT PRIMARY KEY NOT NULL,
    space_id TEXT NOT NULL,
    previous_epoch BIGINT NOT NULL CHECK (previous_epoch >= 0),
    next_epoch BIGINT NOT NULL CHECK (next_epoch > previous_epoch),
    status TEXT NOT NULL CHECK (
        status IN ('prepared', 'staged', 'activated', 'distributing', 'complete', 'recovery_required')
    ),
    encrypted_record BLOB NOT NULL,
    encrypted_stage BLOB,
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL
);

CREATE INDEX idx_member_revocation_log_space_status
    ON member_revocation_log (space_id, status);
