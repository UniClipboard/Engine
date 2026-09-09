# 调查证据

- 本会话此前已复现：20 条 1.713/1.737 秒，67 条 5.033/4.498 秒，空结果 0.205/0.224 秒；运行二进制提交未知。
- 源码 HEAD b5de286ef5d9e02e1831e776eaac939fddb40da9；工作区起始干净。
- ProfileContentKeyVault 每次 resolve 加载完整持久目录；search_catalog 还会再次读取外层 key 派生搜索根。
- ActiveSpaceSecuritySession 已统一 catalog 与 session 安装，先持久化后安装活动会话；恢复路径临时写入共享 session，失败恢复快照。
- RuntimeSpaceAccessAdapter.lock 直接 session.clear；vault 不关联会话。必须确认历史读取是否本来允许锁定后继续，不能盲目绑定活动 Space。
- 033 明确活动会话只决定新写入和网络发送；profile vault 独立于活动 Space MasterKey。
- derived_payloads.rs 的升级验证测试在 session.clear 后调用 inspect_output，证明不能把所有 V3 open 改成要求活动会话。
- vault 有 4 MiB 明文、128 组、每组 1024、总计 4096 key 的现有上限；适合全量有界目录，无需 LRU/TTL。
- persistence.write_atomically 在 rename 后仍有 parent sync 失败窗口；缓存安装失败不能无条件继续信任旧 revision。
- factory reset 先 StopProfileRuntimePort 后删密钥；目前 stopper 停 supervisor/tasks，需要新材料生命周期清理覆盖该入口及 shutdown timeout。
- 未确认同一 profile 是否有全生命周期进程排他锁；只有升级/激活局部 lease 不能证明快照跨实例一致性。
- 少数探索命令使用了不存在的路径与 zsh 空 glob，已改用 rg --files 定位；未产生仓库修改或影响复现。

- 用户新增硬要求：GUI 锁定期间后台接收同步及密文写入继续。最终设计将 GUI 交互授权与后台安全会话分开，GUI 锁定不撤销复用许可。
- 正式设计唯一正文：docs/exec-plans/completed/profile-content-key-runtime-reuse.md；含全部 to-spec 章节、并发矩阵、历史兼容、关闭与未来 GUI 契约。
