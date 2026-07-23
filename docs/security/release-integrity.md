# 发布完整性

## 统一来源

每个 `core-v*` 发布的源码、锁文件、许可证、iOS、Android、HarmonyOS 和调试资料必须来自同一 Git 提交，并共享同一个核心版本。

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

LAN 兼容线使用独立的 `uc-mobile-v*` 标签和工作流，其资产不得混入 `core-v*` 发布。
