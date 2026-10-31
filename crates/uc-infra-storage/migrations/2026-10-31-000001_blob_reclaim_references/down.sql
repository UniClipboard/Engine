DROP INDEX IF EXISTS idx_entry_file_set_blob_id;
DROP INDEX IF EXISTS idx_representation_thumbnail_blob_id;
DROP INDEX IF EXISTS idx_snapshot_representation_blob_id;

ALTER TABLE blob DROP COLUMN last_claimed_at_ms;
