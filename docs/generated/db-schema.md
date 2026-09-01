# 数据库 Schema 快照

> 生成日期：2026-09-01
>
> 权威来源：`crates/uc-infra/src/db/schema.rs` 与 `crates/uc-infra/migrations/`
> 本文件是导航快照；字段、约束和迁移顺序以源码为准。

当前 Diesel schema 声明 28 张表：

| 领域 | 表 |
| --- | --- |
| 剪贴板与 Blob | `active_clipboard_register`, `blob`, `blob_reference`, `clipboard_entry`, `clipboard_event`, `clipboard_selection`, `clipboard_representation_thumbnail`, `clipboard_snapshot_representation`, `entry_file_set` |
| 投递、接收与文件 | `clipboard_entry_delivery`, `directory_publish_log`, `entry_receive_attempt`, `file_transfer`, `file_transfer_events`, `file_transfer_privacy_maintenance`, `receive_artifact_log` |
| 搜索 | `search_document`, `search_entry_tag`, `search_posting`, `search_index_meta` |
| 关系与安全 | `encrypted_relationship`, `relationship_privacy_maintenance`, `space_key_epoch_state`, `member_revocation_log` |
| 兼容、迁移与维护 | `clipboard_migration_backup`, `legacy_space_bootstrap_log`, `legacy_upgrade_pending_join`, `mobile_device` |

部分安全与准入状态通过单独的存储抽象或原始 SQL 管理，不一定出现在 Diesel `schema.rs` 中；因此本表清单
不能用于推断所有物理持久状态。可视化关系图是较早快照，阅读时同时核对最新迁移。
