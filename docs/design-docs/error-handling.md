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

端口错误以 `Box<dyn Error + Send + Sync>` 保存来源时，`anyhow::Error` 必须先加固定动作 context 再转换
（`error.context("固定动作").into()`）。不带 context 直接 `.into()` 得到的是 anyhow 的内部包装类型：显示文本与根错误相同，
却无法 `downcast` 成根错误，分类器因此识别不出 SQLite、IO 等具体类型。std 错误可以直接 `.into()`。

### 允许丢弃来源的情形

以下来源不含可用诊断信息，或不能作为 source 保存，可以使用 `map_err(|_| ..)`，但必须在同一行或前一行用中文注释写明理由：

- 锁中毒 `PoisonError<Guard>`：持有 guard，不能跨线程保存；
- `TryFromIntError`、`TryFromSliceError`：目标分类已完整表达长度或范围不符；
- `tokio::time::error::Elapsed`：超时本身就是分类；
- 通道 `SendError<T>`、`TrySendError<T>`：携带待发负载，保存会延长负载生命周期，且可能含敏感内容；
- `uc-core` 内部纯校验结果改分类，且下层同样是纯校验、没有外部失败。
- 错误值本身不含信息或携带负载：`oneshot`/`mpsc` 的 `RecvError`（只表示对端已退出）、panic 载荷（不是 `Error`）、
  错误类型为 `()`、`Vec` 转定长数组失败（错误值是原字节，可能是密钥或其他敏感内容）、只回显原值的错误
  （`FromStr` 的 `String` 错误、`CapacityError` 等，保存会把原值带进错误链）；
- 宿主或用户输入的纯格式校验（URL、标识、配置值等）：拒绝原因已由目标分类或错误码完整表达；
- 观测运行时自身的初始化失败：只降级为固定的 `SetupStatus`，此时日志通道尚未建立，来源无处记录；
- 失败落为业务结果、不向上传递（如文件集排除行、本轮交换延期）：可丢弃来源，但吞错处必须按
  [运行期观测](observability.md#错误来源与日志字段)以固定分类记录一次；
- 公开契约边界映射：`uc-engine` 的 `EngineError` 与绑定层 `BindingError`、FFI 错误只含稳定错误码、分类与可重试标记，
  不携带来源。失败分类由完整负责人的完成记录（含组装层端口装饰器）从 source chain 提取，边界映射点不再单独记录。
  缺少负责人记录的操作在负责人处补齐（做法见[错误来源保留执行计划](../exec-plans/completed/2026-09-24-error-source-preservation.md)
  E10 记录），不在边界映射点补日志。

同一错误类型既有纯校验失败、又有下层失败时，变体使用 `Variant { source: Option<..> }`，并提供无来源与带来源两个构造函数；
本身不含信息的密码学错误（如 `aead::Error`）同样作为来源保留，不按例外丢弃。

读取持久数据、对端输入或外部系统时，即使只是解析失败，也必须保留来源。

`scripts/architecture/check-rust-style.mjs` 对新增的非测试代码行执行上表检查：拒绝前三种写法，以及同一行和前一行都没有中文注释的
`map_err(|_| ..)`。检查基于文本规则，错误变量按 `e`、`err`、`error`、`source`、`cause` 及 `*_err`、`*_error` 命名识别；
经其他变量名转手的写法仍需审查发现。
现有代码的逐项清理见[错误来源保留执行计划](../exec-plans/completed/2026-09-24-error-source-preservation.md)。

## 安全上下文

`anyhow::Context` 与 source 构造器只能增加固定、脱敏的动作描述；`check-rust-style.mjs` 拒绝在
`with_context`、`anyhow!`、`bail!`、`panic!` 与 serde `custom` 错误文本中出现 `.display()` 路径。不得加入剪贴板内容、密码、密钥、
令牌、设备名、地址、邀请、文件名、文件路径或其他敏感负载。

保留下来的 source chain 只供类型判断与固定分类提取使用，不以 `%error`、`{:#}` 或 `?error` 输出到日志；
日志字段要求见[运行期观测](observability.md#错误来源与日志字段)。

## 测试

新增或修改错误转换时，测试至少验证：

- 对外稳定分类正确；
- `std::error::Error::source()` 非空；
- 适用时，source chain 能追溯到原始下层失败。

只断言显示文本不算完成。
