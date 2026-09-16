# 调查结果

- 当前证据验证后只保存分支 ID、选择资格和来源，未保存成员/原因，查询远端成员只能返回未知。
- 当前 Ledger 存档格式 V2 直接 postcard 编码 LoadedMembershipLedger；增加字段必须显式迁移，不能依赖 serde default 猜测旧字节。
- 预览不授予权限：属于目标组不代表所有成员都已确认同一组，必须单独表达待确认。
- 原因只保留固定类别、关键成员变化及已验证决定，不使用多语言句子或产品翻译 key。
- Desktop 当前投影逐字段复制，新字段需要同步转发，界面翻译和新布局不在本轮实现范围。
- 新影响分成 sync_scope、paused、pending_confirmation：远端选项的范围已知，但恢复尚未完成，待确认列表不能省略；这些字段只用于展示，不授予实际权限。
- 资料缺失返回 members_complete=false、impact=None、reason=unknown；新证据能为旧事项补齐资料，重放相同资料不增加 revision。
- 两次 patch 因格式化后的行不同而未匹配，已读取精确上下文后应用；一次生成脚本语法错误未修改文件。
