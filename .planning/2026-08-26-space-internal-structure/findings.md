# Space 内部结构发现

## 2026-08-26

- 上一阶段已建立 Space 根出口：子模块私有，Space 外部不存在深层路径引用。
- 当前根部仍平铺 20 余个内部模块；后续移动不会再要求 Space 外部模块修改路径。
- 生命周期范围：initialize、unlock、lock、recover、rebuild、reset、upgrade、两个查询、current_space、session。
- 成员范围：current_member_signing、query_membership_admission、query_device_trust、remove、decide、历史收发、membership_ledger、maintain runtime、re_pairing。
- admission 已形成独立目录，保留当前位置；connectivity 的 network recovery 应进入明确的 recovery 子目录。
- 目标结构规则首次运行按预期失败：11 个目标入口尚不存在，21 个旧平铺目录仍存在；根目录白名单同时准确拒绝这些旧目录。
- 生命周期责任区可通过一个 `mod.rs` 同时提供公开契约和 Space 内部协作类型；具体 case 和 session 子模块保持私有。
- 连接恢复本身已经是深模块，只需从单文件改为 `connectivity/recovery/mod.rs`，无需拆分其内部状态机。
- admission 也需要同样的责任区出口：Facade、lifecycle 和 application 只从 `admission/mod.rs` 取协作对象，邀请等子模块保持私有。
- invitation 有六个对象确实需要跨到同属 Space 的其他责任区；它们使用 Space 范围可见性，不公开 invitation 子模块。
