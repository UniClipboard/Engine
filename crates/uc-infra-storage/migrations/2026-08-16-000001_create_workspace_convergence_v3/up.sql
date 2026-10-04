CREATE TABLE workspace_convergence_v3_slots (
    space_lookup_token TEXT NOT NULL,
    slot_id TEXT NOT NULL,
    encrypted_payload BLOB NOT NULL,
    updated_at_ms BIGINT NOT NULL,
    PRIMARY KEY (space_lookup_token, slot_id)
);

CREATE TABLE workspace_convergence_v3_active (
    space_lookup_token TEXT PRIMARY KEY NOT NULL,
    slot_id TEXT NOT NULL,
    generation BIGINT NOT NULL
);

CREATE TABLE workspace_convergence_v3_migrations (
    space_lookup_token TEXT NOT NULL,
    migration_id TEXT NOT NULL,
    encrypted_payload BLOB NOT NULL,
    updated_at_ms BIGINT NOT NULL,
    PRIMARY KEY (space_lookup_token, migration_id)
);

CREATE TABLE admission_repository_state (
    singleton_id INTEGER PRIMARY KEY NOT NULL CHECK (singleton_id = 1),
    encrypted_payload BLOB NOT NULL
);
