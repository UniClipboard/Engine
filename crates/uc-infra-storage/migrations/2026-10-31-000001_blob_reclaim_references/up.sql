-- 删除条目后按引用回收 blob 所需的最终格式。
-- last_claimed_at_ms 是毫秒时间戳，与 created_at_ms 一样不是用户内容或凭据。
-- 已有行先取 created_at_ms，避免把历史 blob 标成从未被认领。
ALTER TABLE blob ADD COLUMN last_claimed_at_ms BIGINT NOT NULL DEFAULT 0;
UPDATE blob SET last_claimed_at_ms = created_at_ms;

-- 三类引用列都没有索引。回收时的 NOT EXISTS 会按 blob_id 回查它们。
CREATE INDEX idx_snapshot_representation_blob_id
    ON clipboard_snapshot_representation (blob_id);

CREATE INDEX idx_representation_thumbnail_blob_id
    ON clipboard_representation_thumbnail (thumbnail_blob_id);

CREATE INDEX idx_entry_file_set_blob_id
    ON entry_file_set (blob_id);
