-- 本机历史标签：用户手动维护的分类定义与条目关联。
--
-- history_tag 是标签定义的权威来源。tag_id 是随机生成的不透明行键，不由名称派生；
-- 名称与创建时间只以 ContentProtection 密文保存在 payload_ct（AAD 绑定 tag_id）。
--
-- history_tag_assignment 是条目关联的权威来源：每个带用户标签的条目一行，该条目的
-- 全部标签 id 只以密文保存在 tags_ct（AAD 绑定 entry_id）；标签集合为空时删除该行，
-- 条目删除时随外键级联删除。
--
-- 搜索索引中的用户标签成员只以搜索密钥 HMAC 后的 posting 存在，并由重建从本表
-- 重新派生；派生索引中不保存明文标签 id。标签与关联不进入任何同步载荷。

CREATE TABLE history_tag (
    tag_id     TEXT PRIMARY KEY NOT NULL,
    payload_ct BLOB NOT NULL
);

CREATE TABLE history_tag_assignment (
    entry_id TEXT PRIMARY KEY NOT NULL REFERENCES clipboard_entry (entry_id) ON DELETE CASCADE,
    tags_ct  BLOB NOT NULL
);
