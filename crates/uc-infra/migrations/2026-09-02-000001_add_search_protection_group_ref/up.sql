ALTER TABLE search_document ADD COLUMN protection_group_ref BLOB;

CREATE INDEX idx_search_document_profile_protection_group
    ON search_document (profile_id, protection_group_ref);
