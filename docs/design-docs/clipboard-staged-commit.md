# 暂存字节与采集提交顺序

剪贴板采集用例是完整流程的唯一负责人。宿主仍只调用完整采集或接收动作，不参与暂存、提交或后台通知。

`SpoolQueuePort.persist` 只持久保存暂存字节。采集用例先等待耐久写入，再提交表示、条目与选择；提交成功后才调用 `notify_committed` 发布后台处理任务。普通创建、替换和入站原子提交使用同一通知位置。后台处理器不得因记录尚未提交而把新任务当作已删除记录结束。

耐久写入失败阻止条目提交。数据库提交失败不发布处理任务。已提交条目即使遇到通知通道满或进程退出，仍保留耐久字节，由既有启动扫描恢复。暂存格式、加密、孤立文件清理、重试与锁定策略均不改变，不增加持久化版本。

端到端验证从公开 Engine 的移动文件上传及图片文档入口进入，使用真实 SQLite、暂存和 blob 后台处理。缩略图必须在首次原图读取之前自行就绪；旧的原图读取触发重新排队行为不能掩盖通知顺序错误。运行：

```bash
just cargo test --locked -p uc-engine --features lan-compat --test host_contract staged_capture_commit -- --nocapture
```

可通过 `UC_STAGED_IMAGE_E2E_RECEIPT` 指定结果 JSON 路径。该场景不接触系统剪贴板、系统钥匙串或真实用户资料。

## Receiver admission and startup recovery

`FileTransferFacade` owns receiver-session admission. Every new receiver session,
including a provisional mobile upload, waits for receive recovery before it reads
the transfer timeline or writes a receiver registration or event. Recovery includes
the SQLite privacy-maintenance checkpoint and `VACUUM`, reconciliation, and staging
cleanup. A host must not coordinate these steps or retry an upload to work around
startup ordering.

Recovery failure prevents registration and retains its complete source chain in
`FileTransferApplicationError::ReceiveRecovery`. The existing readiness coordinator
owns serialization and retry. Admission checks the session registry before recovery
and again under the creation lock afterward. Closing the facade while admission
waits therefore prevents a late session from being created.

Controlled owner-level acceptance holds privacy maintenance to verify entry and
provisional admission, failed recovery without writes, and close during recovery:

```bash
just cargo test --locked -p uc-application --test file_transfer receiver_creation_
```

The public Engine image acceptance above remains the SQLite, staging, blob, and
thumbnail end-to-end check. Set `UC_STAGED_IMAGE_E2E_RECEIPT` to retain its JSON
receipt. A successful retry alone does not prove that an older frozen Engine's
independent intermittent receive failure has been fixed.
