# 密文持久化规则

## 默认规则

任何写入 SQLite、磁盘缓存、搜索索引或配置文件的业务负载，默认必须先经 MasterKey AEAD 加密；其中的用户内容没有例外以外的明文路径。配置按下文「配置与凭据」处理。新增配置以外的字段、表、索引文件或状态文件时，维护者必须先证明它不会写入用户内容，再决定是否可以明文保存。

必须加密的内容包括但不限于：

- 剪贴板正文、标题、预览和富文本；
- 搜索渲染字段和标签名；
- 文件名、文件路径和目录结构，但入站文件在受管文件缓存中的实际文件 basename 除外；
- 用户名、设备备注及其他用户输入；
- 能够还原上述内容的派生值。

## 允许例外

- 内容类型分类枚举，例如 `text`、`image`、`file`、`link`。
- 文件内容本体，可以由 blob store 或核心导入目录按原始字节保存。
- 入站文件在受管文件缓存中的经安全清理的原始文件名，仅限作为实际缓存文件的 basename。

上述例外不包含原始目录路径、数据库/搜索字段、日志、目录名或其他关联元数据。

## Known search index gap

The production search index currently stores `file_extensions` as plaintext JSON in SQLite.
The search projection derives these extensions from file names and paths. This violates the default rule above.
The V3 search path also writes the extensions in plaintext. See [`projection.rs`](../../crates/uc-application/src/search/projection.rs), [`rows.rs`](../../crates/uc-infra-storage/src/search/rows.rs), and [`infra.rs`](../../crates/uc-engine/src/assembly/wire/infra.rs).

The index also stores `mime_type` and `source_device` in plaintext. Their input ranges and protection requirements need review. An arbitrary MIME string is not automatically an approved content-type enum.

This section describes the current implementation, not an approved exception. A repair must cover new writes, index rebuild, existing plaintext, and failure recovery. The current evidence is a code-path inspection; a disk probe for these fields has not been run.

## 配置与凭据

- 配置（用户或程序的偏好与参数，如本机设备名、保存目录、relay 地址、局域网地址）可以明文保存，无需逐项审批；配置不得包含用户内容或凭据，日志与观测规则不变。
- 凭据和密钥材料（MasterKey、passphrase、KEK、relay 访问令牌、鉴权令牌等）只能保存在系统安全存储中，不得写入配置文件、日志或其他普通文件；Engine 内已有兼容例外仅限「运行期密钥材料」的 `vault/profile-secrets-v1`，不得扩大，宿主侧的已有例外由宿主仓库规定。

## 变更要求

涉及持久化的改动必须同时检查新建、升级、回退和失败恢复路径。数据库迁移不能把密文转换为明文，也不能通过删除用户数据解决降级问题。

持久业务密文的保护上下文必须不可变。V3 正常读取通过密文中的随机 `ContentKeyId` 从 profile 加密 key vault 解析实际 `ProtectionGroupId` 和 epoch，purpose 由所属持久化 adapter 固定，不得使用当前活动 `SpaceId` 选择历史密钥。`ProtectionGroupId`、`SpaceId`、purpose 和 catalog 映射不得作为新增明文密文头或索引字段落盘；完整上下文必须参与 AEAD 认证。

按事件批量读取表示时，所有 inline 负载都必须通过对应事件和表示身份的 AEAD 认证后才能交付。任一认证失败则整批返回带完整 source chain 的失败，不能把原密文当业务正文或伪造缺失项。删除及清空在读取失败时保留原条目、接收状态和文件引用，修复原资料后通过原动作重试；日志只记录固定分类和数量。

V1/V2 旧格式只允许在一次性、原子、可恢复的 profile storage upgrade 内读取。升级完成后正常路径只写 V3；切换 Space 复用 profile 数据 generation，不得通过批量解密和重加密历史内容完成安全状态切换。

发布前应使用代表性明文探针运行 `scripts/security/scan-plaintext-probe.sh`。扫描器只报告命中的文件，不输出探针内容。


## 运行期密钥材料

资料实例的独立随机密钥统一保存在 userdata 的 `vault/profile-secrets-v1` 中。该文件使用随机根密钥加密，根密钥再由当前空间口令派生的解锁材料包装；系统安全存储只保留当前资料的一条自动解锁材料。文件头不得包含资料身份、设备身份、密钥名称或恢复进度，密文载荷必须认证完整格式和用途。

旧设备启动时先写入并复读新密文，确认每一份原密钥字节一致后，才清理系统安全存储中的旧条目。清理失败不回退新格式，下次启动继续清理，并通过公开恢复状态报告仍有清理待完成。进行中的空间迁移密钥遵循同一规则。改口令、加入或重置空间后必须重新包装同一随机根密钥，不重新生成历史密钥，不遍历重加密正文。

已有且完整资料的正常启动只从系统安全存储读取一次 KEK。资料恢复先用它校验 keyslot 和独立密钥文件，再向会话恢复交付同一份已验证材料。材料只能交付一次；启动范围结束、取消、挂起、重置或本机 KEK 写入/删除时释放未交付材料。它不是进程期 KEK 缓存，不替代显式口令认证时的安全存储复读和写入验证。挂起后恢复必须重新访问系统安全存储。

缺少或错误的自动解锁材料只允许进入受限恢复状态。用户口令必须先通过现有 keyslot 认证，认证成功后才写回自动解锁材料并打开独立密钥文件；错误口令不得写入。独立密钥文件损坏、格式不支持、存储不可用和普通启动失败不得伪装成口令错误。恢复出厂必须同时删除该文件、自动解锁材料、旧条目和资料状态，并作废由这些密钥保护的派生密文（升级备份的安全记录与记录密钥）；密钥与其保护的密文共用一个生命周期，由出厂重置负责人统一作废，文件副本不属于派生密文，继续保留。

`ProfileContentKeyVault` 是历史内容目录的唯一长期内存所有者。后台安全会话成功激活后，通过不透明、可撤销许可开启有界复用；内容读取与搜索根共享一次经 AEAD 和目录规则认证的加载。目录沿用现有大小/条目上限，热读取不访问文件或系统安全存储；外层 vault key 只在加载/写入局部存在，不另做长期缓存。

同一 vault 的读加载和安装共用 I/O 序列及跨实例文件排他租约。目录内部持有完整冷读取和安装；调用方取消等待不丢弃实际访问，也不提前交还租约，暂停等待真实访问结束。安装只有在持久化成功后发布新视图；实际写入错误或任务中断使旧视图失效，因为文件替换完成后的错误不代表旧目录仍是磁盘事实。运行期代次随开启/撤销变化，旧加载、旧事务快照不能重新发布材料或复活已锁定的会话。索引只复制位置引用，调用方只取得单次操作所需材料，不持有整份目录的共享快照。

真正的安全会话 clear 撤销复用并清理长期材料；维护读取保留既有临时加载语义，不由活动 Space 决定历史密钥。离线升级和 detached session 不取得后台复用许可。shutdown、启动失败/取消和 Factory Reset 通过完整安全生命周期能力封口，终态拒绝新读取/安装；进行中操作已持有的短期副本在完成或取消后清理，不声称能撤回已返回明文。

GUI 锁定与上述安全会话锁定不同：未来 GUI 锁定只撤销宿主交互授权，后台继续验证同步内容并密文持久化，不能因此 clear 密钥或暂停网络。主窗口、Quick Panel、通知和内容响应出口需要由宿主统一授权；本实现不新增 GUI 状态或绕过参数。系统剪贴板自动写入等额外交互策略由未来 GUI 功能规格定义。
