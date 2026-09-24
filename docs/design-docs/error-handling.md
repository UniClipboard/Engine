# 错误处理与转换

## 稳定分类

Application 对依赖、存储、网络、系统或密码能力失败进行稳定分类时，错误 variant 必须使用
`#[source] source: anyhow::Error`，或携带另一个实现 `std::error::Error` 的具体 source。
构造方式遵循 `crates/uc-application/src/error.rs`，不得为了 `Clone`、`Copy`、`Eq` 或简化匹配丢弃来源。

纯业务判断在没有下层异常时可以返回普通枚举或明确结果，不得伪造 `anyhow::Error`。一旦失败来自被调用能力，
必须保留完整 source chain 与 backtrace。

## 转换所有权

- 优先实现 `From<LowerError>` 并使用 `?`。
- 转换实现归目标错误所在模块所有；来源错误模块不得反向依赖上层错误。
- 只有改变语义分类或补充安全动作上下文时才使用 `map_err`。
- 禁止字符串化或吞掉来源：`Error::X(error.to_string())`、`map_err(|_| Error::X)`、无来源 unit variant 均不合规。

### 字符串化的常见形式

以下写法都会丢失具体错误类型，使调用方无法按类型判断、日志分类器无法提取固定原因：

| 反模式 | 替换写法 |
| --- | --- |
| `.map_err(\|e\| anyhow!(e.to_string()))`、`anyhow::Error::msg(e)` | 直接 `?`，或 `.map_err(anyhow::Error::new)` |
| `.map_err(\|e\| anyhow!("动作: {e}"))` | `.context("固定动作")` |
| `Error::X(e.to_string())`、`Error::X(format!("..{e}"))`、`reason: e.to_string()` | 变体携带 `#[source]` 具体错误或 `anyhow::Error`，优先 `From` + `?` |
| `.map_err(\|_\| Error::X)` 丢弃被调用能力的错误 | 变体携带 `#[source]` |

`context` 文本只写固定动作，不拼接错误正文、路径、标签值或内容。

### 允许丢弃来源的情形

以下来源不含可用诊断信息，或不能作为 source 保存，可以使用 `map_err(|_| ..)`，但必须在同一行或前一行用中文注释写明理由：

- 锁中毒 `PoisonError<Guard>`：持有 guard，不能跨线程保存；
- `TryFromIntError`、`TryFromSliceError`：目标分类已完整表达长度或范围不符；
- `tokio::time::error::Elapsed`：超时本身就是分类；
- 通道 `SendError<T>`、`TrySendError<T>`：携带待发负载，保存会延长负载生命周期，且可能含敏感内容；
- `uc-core` 内部纯校验结果改分类，且下层同样是纯校验、没有外部失败。

读取持久数据、对端输入或外部系统时，即使只是解析失败，也必须保留来源。

`scripts/architecture/check-rust-style.mjs` 对新增的非测试代码行执行上表检查：拒绝前三种写法，以及同一行和前一行都没有中文注释的
`map_err(|_| ..)`。检查基于文本规则，错误变量按 `e`、`err`、`error`、`source`、`cause` 及 `*_err`、`*_error` 命名识别；
经其他变量名转手的写法仍需审查发现。
现有代码的逐项清理见[错误来源保留执行计划](../exec-plans/active/2026-09-24-error-source-preservation.md)。

## 安全上下文

`anyhow::Context` 与 source 构造器只能增加固定、脱敏的动作描述。不得加入剪贴板内容、密码、密钥、
令牌、设备名、地址、邀请、文件名、文件路径或其他敏感负载。

保留下来的 source chain 只供类型判断与固定分类提取使用，不以 `%error`、`{:#}` 或 `?error` 输出到日志；
日志字段要求见[运行期观测](observability.md#错误来源与日志字段)。

## 测试

新增或修改错误转换时，测试至少验证：

- 对外稳定分类正确；
- `std::error::Error::source()` 非空；
- 适用时，source chain 能追溯到原始下层失败。

只断言显示文本不算完成。
