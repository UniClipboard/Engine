# 发布完整性

## 统一来源

每个 `v*` 发布的源码、锁文件、许可证、iOS、Android、HarmonyOS 和调试资料必须来自同一 Git 提交，并共享同一个 Engine 版本。

发布流程先生成各平台产物，再由 `scripts/release/build-release-manifest.mjs` 计算文件大小和 SHA-256，最后由 `scripts/release/verify-release-bundle.mjs` 重新计算并核对。校验完成前不得创建 GitHub Release。

## 必需资产

- iOS XCFramework、Swift 绑定和 SwiftPM 校验值；
- Android AAR、Kotlin 绑定、POM 和运行依赖；
- HarmonyOS HAR、ARM64 动态库、ArkTS 声明和已签名验收 HAP；
- 源码归档、`Cargo.lock` 和许可证清单；
- 三端调试资料；
- `release-manifest.json`。

## 设备矩阵

设备矩阵的状态只能是 `passed`、`failed` 或 `skipped`。`skipped` 必须说明原因，生成器和核验器都禁止把它转换成 `passed`。

## 不可变性

发布前必须确认同名标签和 Release 不存在。资产上传后不得覆盖；发现错误时发布新版本，并在旧版本说明中标记问题。

LAN 兼容线使用独立的 `uc-mobile-v*` 标签和工作流，其资产不得混入 `v*` 发布。

## 发布后采用

正式发布成功后，流程使用上一版和新版分别运行两个桌面节点，交换新旧角色完成配对，并在两个方向核对实际收到的内容。试运行、发布失败或联通检查失败时不得通知产品仓库。

联通检查通过后，Engine 使用组织安装的 GitHub App 向桌面端和移动端发送版本号与完整源码提交。两个产品仓库必须重新读取公开发布清单并独立核对，不得直接信任通知中的产物信息。

GitHub App 仅安装到 `UniClipboard`、`UniClip` 两个目标仓库，仓库权限只开放“元数据：只读”“内容：读写”和“拉取请求：读写”。Engine 用它触发两个产品仓库，产品仓库再用同一个 App 推送固定版本分支并创建或更新拉取请求。组织级 Actions Variable `ENGINE_RELEASE_APP_CLIENT_ID` 和 Actions Secret `ENGINE_RELEASE_APP_PRIVATE_KEY` 只向 Engine 及两个产品仓库开放；不得使用个人访问密钥。
