# Peer Address 读取失败的准确诊断

## 状态

- **状态**：实施完成；等待下游产品构建采集用户现场的新诊断
- **日期**：2026-09-22
- **来源问题**：Desktop t-0044 在 Engine `v1.1.0-rc.18` 恢复本地会话时连续出现 `address.record.read_failed -> address_unavailable -> session recovery failed`，现有记录不能区分数据库、未解锁、密文认证、载荷版本或解码失败
- **相关调查**：t-0044 是 peer-address 关系读取失败；t-0031 / PR #110 是 admission repository 读取失败。两者发生阶段不同，不能因同一启动错误页认定同根因
- **完整负责人**：Infra 的 `EncryptedRelationshipStore` 负责在真实失败产生处保留来源和分类；`DieselPeerAddressRepository` 负责把一次地址读取的稳定诊断交给既有本地观测合同；Application 与 Engine 继续负责原会话恢复和启动安全结果
- **调用方唯一动作**：调用方继续只调用一次 `PeerAddressRepositoryPort::get` 或既有会话恢复，不查询内部步骤、不拼接分类或栈
- **成功结果**：正常地址读取结果不变；失败时内部错误链保留真实下层 source，本地导出包含稳定类别、发生阶段、固定来源链和一次捕获的脱敏真实 Rust 符号栈
- **失败结果**：不能安全细分时明确为 `unknown`；诊断采集或符号解析不足不得改变原业务错误和启动结果
- **重试与重启责任**：不改变现有责任。会话恢复与启动仍按当前安全策略失败；本计划不增加自动删除、跳过、重建或降级启动

## 范围

### 目标

- 区分代码能够真实证明的 `storage`、`locked`、`authentication`、`unsupported_version`、`payload_decode` 与 `unknown`。
- 在关系存储首次构造分类错误时保留下层 source，并捕获一次 backtrace；上层只转交，不重复抓栈。
- 地址读取本地诊断输出固定 `error.category`、`error.stage`、`error.chain`、`error.stack` 与栈捕获状态；不输出错误正文。
- 栈只保留符号行并限制深度；删除源码绝对路径、文件名、行号、地址、设备/空间标识、密文和其他动态值。
- `PeerAddressError` 继续提供稳定外部类别并携带 source，Engine 最终仍只向用户暴露合适的稳定启动类别。

### 非目标

- 不修改持久化格式、数据库 schema、地址载荷版本或设备间协议。
- 不读取真实 profile、Keychain 或 t-0044 原始资料，不推断用户现场的精确子原因。
- 不删除坏记录、不跳过关系、不重建资料、不放宽 rc.18 的启动安全策略。
- 不把静态手写模块路径称为调用栈，不输出原始 `Debug`、`Display`、SQL 或 backtrace 文本。
- 不为诊断新增 Engine facade、Application port、公开恢复动作或产品 UI。

## 最小端到端切片

从隔离临时 SQLite 与虚构 profile/space 构造真实 `EncryptedRelationshipStore -> DieselPeerAddressRepository -> uc.connectivity JSONL` 路径：先验证一条认证失败能保留 source、分类和真实符号栈并进入最终导出，再扩展其余可证明类别。正常密文往返作为对照，确认没有额外失败字段且行为不变。

## 失败矩阵

| 注入 | 预期类别 | 阶段 | 证据边界 |
| --- | --- | --- | --- |
| SQLite 查询失败 | `storage` | `database_read` | 保留 Diesel/执行器 source，不输出 SQL/路径 |
| 地址读取前迁移查询失败 | `storage` | `migration_read` | 保留 Diesel/执行器 source，明确失败发生在迁移前置路径 |
| Space 未解锁 | `locked` | `key_derivation` | 保留 `SpaceAccessError` source |
| AEAD 认证失败 | `authentication` | `ciphertext_authentication` | 无法进一步证明哪一字节或哪一密钥错误 |
| 关系 envelope 版本不支持 | `unsupported_version` | `envelope_decode` | 只说明格式版本不可用，不输出版本值 |
| 已认证明文无法解码或载荷版本不支持 | `payload_decode` / `unsupported_version` | `payload_decode` | JSON 语法失败与载荷版本分别分类 |
| 未匹配的安全错误 | `unknown` | 对应边界 | 不伪造精度 |

## 验证

- 相关真实存储测试：正常往返、上述失败矩阵、`Error::source()` 链和一次栈捕获。
- 端到端诊断工件：生成隔离 JSONL，自动断言固定字段、真实符号栈、禁止字段和虚构敏感标记均未泄漏；工件复制到线程 library。
- 正常路径：地址保存/读取和 Iroh 地址二次解码保持原结果。
- 门禁：相关 crate 测试、`cargo metadata --locked`、workspace check、fmt、Rust style、Engine repository、观测隐私及 `git diff --check`。
- 平台限制：Rust backtrace 在 release 构建可捕获，但符号完整度取决于目标平台的符号表、内联和剥离设置；没有符号时必须报告 `unresolved`，不能生成静态替代栈。实体 macOS/Windows/iOS/Android 产品构建本计划不执行，均记为跳过。

## 实施结果

- peer-address 读取失败在 `EncryptedRelationshipStore` 首次可证明原因处分类并保留 source；`PeerAddressError` 只跨层携带稳定类别、阶段和 source，不携带观测栈。
- `address.record.read_failed` 保持原事件名，并新增固定诊断字段。动态错误正文、SQL、地址、标识和源码路径不进入导出。
- 隔离端到端测试覆盖 `storage`、`locked`、`authentication`、`unsupported_version`、`payload_decode` 和 `unknown` 六类，并额外验证地址读取前迁移查询失败输出 `storage/migration_read`，生成 `.herdr-project/uni-t-0045/library/peer-address-read-diagnostics.json`。
- 栈过滤会丢弃 backtrace 捕获器自身、源码位置和无关运行时帧；只有实际捕获到关系存储或 peer-address 仓储符号时才标记 `captured`。本机 `--release` 隔离验证中认证、载荷解码、版本和迁移查询场景保留业务符号；未解锁、常规数据库读取和未知密钥错误因优化后符号不足输出空栈和 `unresolved`。
- 相关仓储回归、观测合同、workspace check、格式、架构、隐私和 diff 检查通过；已执行本机 Cargo release 隔离验证，物理设备与正式产品发布构建均未执行。
