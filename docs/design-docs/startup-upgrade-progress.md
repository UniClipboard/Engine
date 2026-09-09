# 启动资料升级进度

状态：只读启动进度已实现；本批不包含启动前密码输入。

已执行的检查与未验证范围见[交付记录](../exec-plans/completed/startup-upgrade-progress.md)。

## 责任与边界

Engine 负责一次完整启动的最终结果；`ProfileStorageUpgrade` 继续负责资料格式转换、计量、校验、互斥与持久恢复。
宿主只调用一次启动，不解析恢复记录、不查询内部步骤、不打开数据库，不根据产品进度决定恢复行为。
进度属于本地产品能力，不新增日志、远程属性或内容标识。只含枚举、数量、一次启动随机标识与序号。

## 接入

原有 `Engine::start(config, host)` 保持兼容。新接入先调用 `StartupProgress::channel()`，取得单次消费的
`StartupProgressInput` 和可克隆的 `StartupProgress`，再调用 `Engine::start_with_progress(config, host, input)`。
输入不能克隆；每次重试创建新通道。窗口持有观察者，后台持有并执行启动任务，窗口关闭不取消启动任务。

```rust,no_run
use uc_engine::{Engine, EngineConfig, HostCapabilities, StartupProgress};

async fn start(config: EngineConfig, host: HostCapabilities) {
    let (input, mut progress) = StartupProgress::channel();
    let latest = progress.snapshot();
    let startup = tokio::spawn(Engine::start_with_progress(config, host, input));
    // 后台状态服务保留 progress 的副本，窗口订阅可随时断开。
    while let Some(snapshot) = progress.changed().await {
        // 将 snapshot 更新到宿主已认证的本地状态服务。
        if snapshot.state.is_terminal() {
            break;
        }
    }
    let result = startup.await;
    // result 才是完整 Engine 或启动错误；失败后仍可 progress.snapshot()。
}
```

观察者的 `snapshot()` 返回自有快照；`changed().await` 返回合并后的新快照，通道结束后返回 `None`。
重新连接先读快照再等变化。宿主按尝试标识和单调序号去重，不把旧尝试的更新覆盖新尝试。
所有观察者共享有界最新状态，步骤记录按有限类别替换保留，没有按内容数量增长的事件队列。
启动返回失败后，观察者仍可读最后结果；启动 future 被宿主丢弃或取消时记录 Interrupted。
进程被系统强制结束时无法发布终态，宿主必须结合进程存活判断，不能把旧快照当作活跃工作。

## 产品含义

- Preparing：准备启动或检查是否需要升级；无需升级不强制展示升级页面。
- Upgrading：确实需要资料升级，当前步骤和有限已完成记录可读。
- StartingServices：资料准备完成，但 Engine 尚不可用。
- Ready：完整 Engine 已构造并返回；这才是宿主进入应用的条件。
- Failed / Interrupted：本次尝试已结束，保留最后步骤和安全失败分类。

内容表示不是历史条数；大块内容不是附件文件数，图片也可能存为大块内容。
附属记录包括文件清单、搜索预览和其他受保护引用，不按历史聚合。
已处理量表示本步骤实际完成处理的输入量，包含已明确保留的不可读内容；警告数单独表达不可用部分。
只有总量可靠时提供总数。校验、复制和准备可保留未知总量；禁止固定权重、总体假百分比或推测剩余时间。
步骤的完成标记由完整负责人在相应验证和耐久边界后发布，计数达到总量本身不代表步骤完成。

## 恢复与失败

既有恢复记录仍是唯一事实来源。恢复可以重做未提交转换；本次计数从实际工作重新建立，不承诺逐条续传。
已耐久部分从已认证的恢复记录重建摘要，当前输出仍由既有恢复流程重新校验；警告从已有输出与来源差异计算，不能因重复验证累加。
已确认的保留内容/不可用记录数量随加密 journal 保存，属于转换结果，不是进度恢复指令。
保持原 postcard 字节前缀，使用经过同一 AEAD 认证的版本化尾部保存两类数量；旧读取器仍能读取前缀，
新读取器接受没有尾部的旧 journal。旧记录没有警告证据时显示未知，不能伪造零；损坏尾部失败关闭。
慢观察者、观察者断开或窗口关闭不改变事务结果。原任务结束后才允许宿主新建重试；跨进程存储租约继续拒绝竞争执行者。

类型化 I/O 的磁盘空间、权限、资料损坏、来源变化和保护材料不可用分别表达。无法从类型化错误证明的情况保守归类，
不解析英文错误消息，不把保护材料缺失猜成密码错误。
既有 SQLite/系统 adapter 丢失具体原因时返回 `StorageUnavailable` 或 `ProtectionUnavailable`，不承诺仅凭此摘要定位所有系统失败。
本批没有启动前密码输入通道，也不提供解锁动作。保护材料恢复需在原环境或既有恢复入口完成后重新启动；
后续密码能力必须验证哪些材料确实可由密码恢复及密码生命周期，不能删除保护记录或生成替代密钥绕过。

## 宿主与后续责任

Desktop 负责启动前认证状态服务、跨窗口快照与订阅、12/45 秒等待规则、进程检测及就绪后自动连接。
本仓不会通过正常运行事件流代替启动前状态，也不新增 Desktop 或 CI 改动。
移动绑定继续使用原入口；新增绑定支持和启动前解锁属于后续独立工作，不能宣称本批已支持。
