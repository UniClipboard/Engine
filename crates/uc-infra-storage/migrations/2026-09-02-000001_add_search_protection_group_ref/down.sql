DROP INDEX IF EXISTS idx_search_document_profile_protection_group;

ALTER TABLE search_document DROP COLUMN protection_group_ref;
