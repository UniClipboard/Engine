-- Relationship encryption is intentionally forward-only. Restoring the old
-- plaintext tables would either expose user data or silently discard records.
SELECT 1 FROM "cannot downgrade: encrypted relationships are forward-only";
