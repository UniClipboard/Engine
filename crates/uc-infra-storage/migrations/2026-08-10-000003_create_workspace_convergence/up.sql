CREATE TABLE workspace_convergence_state (
    space_lookup_token TEXT PRIMARY KEY NOT NULL,
    encrypted_payload BLOB NOT NULL,
    updated_at_ms BIGINT NOT NULL
);
