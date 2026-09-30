# Findings
- 审计（grep 近似）：instrument 131（23 无 skip_all，13 带 err）；插值 #[error] 123；日志字段 %/? 63；宏调用约 880；derive(Error) 约 325。
- spike：anyhow context 层不可 downcast；无 context 的 anyhow→Box 首层是 anyhow 包装且不可识别；`SpaceAdmissionStateStoreError` 与 io::Error 可 downcast。
- ObservabilityConfig 是 pub 字段结构体（无 non_exhaustive），不得加字段；D 用 cfg(debug_assertions)/feature。
- t-0114 报告：其 Sponsor 改动 uc-engine 编译失败、未通过交付检查；不移植、不还原该工作区。
