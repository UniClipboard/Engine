# 删除条目时回收无引用 blob

状态：实施中。任务 WF-engine-39 / T-002，对应 issue #39 的第一段。本分支只做删除和替换当时的立即回收，不做孤儿扫描。

## 负责人

`DeleteClipboardEntryUseCase` 负责删除、清空、保留、cleanup 和 reconcile 在删除成功后的回收。捕获与入站提交负责替换路径。调用方唯一动作是删除或替换。回收失败只警告，不改变删除结果。文件删不掉留给后续扫描。

## 正确性

数据库在 `BEGIN IMMEDIATE` 里执行 `DELETE FROM blob WHERE blob_id=? AND NOT EXISTS(三类引用)`。去重命中在同一事务确认行仍在并更新 `last_claimed_at_ms`。行已经不在就重新写文件，不复用已删除的 id。

`last_claimed_at_ms` 是毫秒时间戳，与 `created_at_ms` 同类，不是用户内容或凭据。删除语句不看这个时间，避免挡住复制后立刻删除。

## 进程内租约

TTL 15 分钟，按 profile 在同一进程共享。它只减少同进程竞争。新插入的 blob 在返回前释放，因为那时还没有条目引用。去重命中的已有 id 保持到引用落库后由捕获或后台物化释放。该时长需实测确认。

## 非目标

不扫描已经泄漏的孤儿 blob，不改移动端锁释放窗口，不改 CI。
