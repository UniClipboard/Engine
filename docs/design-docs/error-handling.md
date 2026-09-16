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

## 安全上下文

`anyhow::Context` 与 source 构造器只能增加固定、脱敏的动作描述。不得加入剪贴板内容、密码、密钥、
令牌、设备名、地址、邀请、文件名、文件路径或其他敏感负载。

## 测试

新增或修改错误转换时，测试至少验证：

- 对外稳定分类正确；
- `std::error::Error::source()` 非空；
- 适用时，source chain 能追溯到原始下层失败。

只断言显示文本不算完成。
