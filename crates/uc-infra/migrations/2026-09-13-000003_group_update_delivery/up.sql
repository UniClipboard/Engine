CREATE TABLE group_update_delivery (
    lookup_token BLOB PRIMARY KEY NOT NULL,
    space_lookup_token TEXT NOT NULL,
    encrypted_metadata BLOB NOT NULL,
    encrypted_payload BLOB NOT NULL
);

CREATE INDEX group_update_delivery_space_idx
ON group_update_delivery (space_lookup_token);

CREATE TABLE group_update_source (
    source_kind TEXT NOT NULL,
    source_id TEXT NOT NULL,
    encrypted_summary BLOB NOT NULL,
    PRIMARY KEY (source_kind, source_id)
);

CREATE TRIGGER invalidate_space_delivery_summary_after_update
AFTER UPDATE OF encrypted_payload ON space_key_epoch_state
BEGIN
    DELETE FROM group_update_source
    WHERE source_kind = 'space_material' AND source_id = OLD.space_lookup_token;
END;

CREATE TRIGGER invalidate_space_delivery_summary_after_delete
AFTER DELETE ON space_key_epoch_state
BEGIN
    DELETE FROM group_update_source
    WHERE source_kind = 'space_material' AND source_id = OLD.space_lookup_token;
END;

CREATE TRIGGER invalidate_revocation_delivery_summary_after_update
AFTER UPDATE OF encrypted_record, encrypted_stage ON member_revocation_log
BEGIN
    DELETE FROM group_update_source
    WHERE source_kind = 'revocation' AND source_id = OLD.revocation_id;
END;

CREATE TRIGGER invalidate_revocation_delivery_summary_after_delete
AFTER DELETE ON member_revocation_log
BEGIN
    DELETE FROM group_update_source
    WHERE source_kind = 'revocation' AND source_id = OLD.revocation_id;
END;
