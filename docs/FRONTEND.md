# 前端与宿主边界

本仓库不拥有产品前端、页面状态或交互文案。桌面、iOS、Android 和 HarmonyOS 产品通过
`uc-engine` 的稳定操作、结果、事件和宿主能力接入，不能依赖内部 crate 或内部源码路径。

宿主可以负责：

- 私有目录、安全存储、系统剪贴板、文件句柄和生命周期桥接；
- 将稳定 Engine 状态转换成平台 UI；
- 根据用户明确选择启用 LAN 兼容线。

宿主不能负责：

- 复制成员、准入、加密、重试或恢复业务流程；
- 从本地缓存拼装第二份成员状态；
- 因 P2P 失败自动回退 LAN；
- 记录或展示内部错误文本中的敏感数据。

接口细节见 [uc-engine 跨平台核心接口](design-docs/uc-engine-interface.md)，产品语义见
[产品规格](product-specs/)，安全边界见 [SECURITY](SECURITY.md)。
