# UniClipboardEngine 架构

本文件是架构入口，不复述所有实现细节。当前事实的详细说明位于
[`docs/architecture/architecture-bible.md`](docs/architecture/architecture-bible.md)，稳定接口以
[`docs/design-docs/uc-engine-interface.md`](docs/design-docs/uc-engine-interface.md) 与代码为准。

## 系统边界

`UniClipboardEngine` 是 macOS、Windows、Linux、iOS、Android 和 HarmonyOS 共用的端到端加密
P2P 核心。宿主提供私有目录、安全存储、系统剪贴板、文件句柄和生命周期能力；Engine 拥有协议、
持久化、加密、数据库迁移、连接、传输和后台任务。

外部 Rust 使用方只依赖 `uc-engine`。移动绑定只依赖这个稳定入口，并与它使用同一版本。

## 依赖方向

```text
host / bindings
       ↓
   uc-engine          稳定契约、生命周期、组装
       ↓
    uc-infra          数据库、密码、文件、P2P adapter
       ↓
 uc-application       完整业务流程、恢复与稳定分类
       ↓
    uc-core           领域规则、状态机和值对象
```

Port 归需要能力的层所有，因此 Infra 既可以实现 Core port，也可以实现 Application port。
具体 adapter 只在 Engine composition root 中出现。

## 主要模块

| 路径 | 责任 |
| --- | --- |
| `crates/uc-core/` | 与平台无关的领域规则、状态机和值对象 |
| `crates/uc-application/` | 用户/系统动作的完整流程、持久恢复和能力 port |
| `crates/uc-infra/` | SQLite、加密、文件、搜索、Iroh 和系统能力实现 |
| `crates/uc-engine/` | 唯一稳定 Rust 入口、生命周期、运行期与依赖组装 |
| `bindings/` | iOS、Android、HarmonyOS 的薄语言绑定 |
| `compatibility/` | 用户显式启用、独立版本与发布的 LAN 兼容线 |
| `tests/hosts/` | 平台验收宿主，不承载产品业务 |

## 不变量

- 业务持久化默认使用 MasterKey AEAD；明文例外见 [`docs/SECURITY.md`](docs/SECURITY.md)。
- P2P 是默认能力，失败时不自动切换 LAN。
- 一个跨层行为只有一个完整负责人；调用方不编排内部步骤。
- 成员资格来自已验证成员历史，不从在线、地址、缓存或旧关系反推。
- 正式提交后的网络/安全效果由持久阶段恢复，不回滚已提交业务事实。
- 观测由 Engine 组装层的领域 port decorator 完成，不侵入 Application 业务顺序。
- 发布只使用带来源提交、版本、大小和 SHA-256 的 GitHub Release 产物。

## 深入阅读

- [设计文档索引](docs/design-docs/index.md)
- [核心信念](docs/design-docs/core-beliefs.md)
- [Space Application 设计与代码地图](docs/design-docs/space-application.md)
- [执行计划](docs/PLANS.md)
- [可靠性](docs/RELIABILITY.md)
- [安全](docs/SECURITY.md)
- [领域词表](docs/references/domain-glossary.md)
