# 删除条目时回收无引用 blob

状态：实施中。任务 WF-engine-39 / T-002，对应 issue #39 的第一段。本分支只做删除和替换当时的回收，不做孤儿扫描。仍在认领宽限内的 blob，以及文件删不掉的残留，留给 PR2（WF-engine-39 GC sweep）。

## 负责人

`DeleteClipboardEntryUseCase` 负责删除、清空、保留、cleanup 和 reconcile 在删除成功后的回收。捕获与入站提交负责替换路径。调用方唯一动作是删除或替换。回收失败只警告，不改变删除结果。文件删不掉留给 PR2（WF-engine-39 GC sweep）。

## 正确性

跨进程正确性由三项共同保证：

1. 数据库在 `BEGIN IMMEDIATE` 里执行 `DELETE FROM blob WHERE blob_id=? AND NOT EXISTS(三类引用)`。
2. 同一事务还要求 `last_claimed_at_ms` 早于现在减去认领宽限（60 分钟，需实测确认）。宽限内的无引用 blob 记入 `skipped_recently_claimed`，留给 PR2（WF-engine-39 GC sweep）。
3. 写入 `entry_file_set.blob_id` 时在同一事务里确认 blob 行仍在。不存在则返回明确错误并保留 source chain。表示写入仍靠已有外键。

去重命中在立即事务里确认行仍在并更新 `last_claimed_at_ms`。行已经不在就重新写文件，不复用已删除的 id。

`last_claimed_at_ms` 是毫秒时间戳，与 `created_at_ms` 同类，不是用户内容或凭据。新插入的 blob 把它初始化为插入时间，因此「复制后立刻删除」会落在宽限内，本段不立刻回收。正确性优先。

## 进程内租约

TTL 15 分钟，按 profile 在同一进程共享，并按持有者计数。守卫在 `Drop` 时释放；去重命中的已有 id 保持到引用落库或写入失败。它只减少同进程竞争，不替代上面的三项。该时长需实测确认。

## 非目标

不扫描已经泄漏的孤儿 blob，不改移动端锁释放窗口，不改 CI。`inbound_receive_commit_repo` 仍在 Infra 层触发回收，留待后续。
