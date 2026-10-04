CREATE TABLE legacy_upgrade_pending_join (
    peer_lookup_token TEXT PRIMARY KEY NOT NULL,
    encrypted_payload BLOB NOT NULL,
    updated_at_ms BIGINT NOT NULL
);
