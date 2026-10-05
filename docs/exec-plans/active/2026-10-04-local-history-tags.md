# 本机历史标签

状态：本地实现与端到端验收完成，等待合并（未 push、未开 PR）。

## 背景与决定

桌面历史窗口需要用户自建标签：创建、列出、给一条或多条记录添加/移除、按标签搜索与计数、
改名、合并和删除。2026-10-04 用户决定标签与记录关联**按每台设备的本机历史管理**，不新增
同步协议，也不跨设备传播。颜色由宿主按标签 id 确定性映射，Engine 不保存颜色或图标。

## 负责人与动作

| 关注点 | 负责人 |
|---|---|
| 名称规则（trim、NFC、长度、控制字符、忽略大小写的同名键）与上限 | `crates/uc-core/src/clipboard/history_tag.rs` |
| 流程：全部写入串行化、同名判定、批量与合并约束、写后索引同步、排序、结果整理 | `crates/uc-application/src/clipboard/history_tags/` |
| 会话锁定前置检查 | `AppFacade`（与搜索计数一致） |
| 权威存储：名称、创建时间与关联全部 AEAD 密封，单事务写入 | `crates/uc-infra-storage/src/db/repositories/history_tag_repo.rs` |
| 搜索索引中的 HMAC 标签成员、过滤与替换 | `crates/uc-infra-storage/src/search/`（`pipeline.rs`、`sqlite_index.rs`） |
| 结果 `tags` 与用户标签计数补齐、重建/修复/实时索引补齐成员 | `crates/uc-application/src/search/` |
| Operation、结果与错误码 | `crates/uc-engine/src/operations/history/tags.rs` |

调用方唯一动作是对应的 Engine Operation；成功与失败结果见
[uc-engine 接口](../../design-docs/uc-engine-interface.md)。所有写操作是单个 SQLite 事务，不存在需要重试或
重启补偿的中间状态；失败后由宿主重新读取列表决定下一步。

## 持久化

新增迁移 `2026-10-04-000001_create_history_tag`（本分支唯一新格式）：

- `history_tag(tag_id, payload_ct)`：名称与创建时间经 `ContentProtection` 密封，AAD 为
  `uc:history_tag:v1|{tag_id}`。
- `history_tag_assignment(entry_id, tags_ct)`：每个带用户标签的记录一行，该记录的全部标签 id
  经 `ContentProtection` 密封，AAD 为 `uc:history_tag_assignment:v1|{entry_id}`；集合为空时删除该行，
  记录删除时随外键级联删除。
- 两表加入 `PROFILE_DATA_TABLES`；旧格式升级时为空表。
- 搜索索引：用户标签成员只作为保留词项（`history_tag_token`）的 HMAC posting 保存，`field_mask`
  为 `SEARCH_FIELD_HISTORY_TAG`，与关键词共用按保护组派生的搜索密钥；过滤时按索引中的保护组派生
  查询词项。派生的 `search_entry_tag` 不含用户标签。重建、修复与实时索引都从权威关联补齐成员；
  写入后流程立即替换受影响记录的标签 posting（经变更门禁），失败由重建补齐。现有索引没有用户
  标签，不提升 `index_version`。

明文边界（按 [密文持久化规则](../../security/encrypted-persistence.md) 属于脱敏，不是明文例外）：

- `tag_id`：随机 UUID 行键，与 `entry_id` 同类，不由名称派生。
- 每个带用户标签的记录存在一行关联密文（暴露“该记录有用户标签”）。
- 标签 posting 的 HMAC 值在同一保护组内相同，暴露记录之间“共享同一标签”的关系，与
  ADR-013 下关键词 posting 的性质相同。

名称、创建时间和“哪条记录有哪个标签”均不以明文保存。

## 失败方式

| 情况 | 结果 | 证据 |
|---|---|---|
| 空白、控制字符或超长名称 | 1401，不写入 | E2E |
| 大小写、空白或 NFC 形式不同的同名创建 | 返回已有标签，`created = false` | E2E |
| 并发写入（同名创建、关联改写） | 全部标签写入经流程互斥串行化 | 源码 |
| 未知标签的关联、移除、改名、删除 | 1402，整体不变 | E2E |
| 批量中部分记录不存在 | 存在的记录在一个事务内生效，缺失记录返回 | E2E |
| 重复关联或移除 | `changed = 0`，`unchanged = n` | E2E |
| 改名与其他标签同名 | `NameConflict`，不写入 | E2E |
| 合并来源与目标相同或含未知标签 | 1401 / 1402，整体不变 | E2E |
| 重复合并 | 1402（来源已不存在），状态已是最终结果 | E2E |
| 合并时记录已同时携带两者 | 计入 `already_on_target`，只保留一条关联 | E2E |
| 进程在写入中途终止 | 事务回滚，只有操作前或操作后状态；索引不一致由重建补齐 | 源码（单事务） |
| 删除记录 | 外键级联删除关联，计数下降 | E2E |
| 删除标签 | 关联删除，记录内容哈希不变 | E2E |
| 索引重建或版本变化 | 用户标签不在派生索引中，重建后过滤与计数一致 | E2E（显式重建） |
| 重启 | 标签、名称、关联恢复且名称可解密 | E2E |
| 会话锁定 | 全部标签操作 1405 | E2E |
| 名称密文无法打开 | 该行 `name = None` 并记录日志，其余行正常，可删除 | 源码 |
| 旧格式 profile | 1406 | 源码 |
| 索引未就绪时的降级浏览 | 从权威存储补齐用户标签 id | 源码 |
| 关联密文无法打开 | 该记录按无用户标签处理并记录日志 | 源码 |
| 保护组轮换后条目文档与标签 posting 所属组不同 | 过滤按索引中全部保护组派生查询词项，与关键词 posting 相同 | 源码（未做 E2E） |
| 写入后索引更新失败 | 权威存储已提交，记录日志，重建补齐 | 源码 |
| 派生索引、posting 或关联密文中出现明文标签 id | 不出现 | E2E SQL 检查 |
| 资料目录（数据库、WAL、日志）中出现名称明文 | 不出现 | E2E 扫描 |

## 验收

- `cargo test -p uc-engine --test history_tags_lifecycle`：真实 Engine、真实 SQLite、真实加密会话，
  覆盖上表标注为 E2E 的各项。
- 未实现、未需：相似标签建议、颜色/图标、自动规则标签、跨设备同步、合并/删除撤销、移动端绑定。

## 剩余事项

- Desktop t-0167 以本地 Engine override 联调；跨仓发布前替换为已合并的不可变来源。
