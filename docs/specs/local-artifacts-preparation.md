# 本地产物准备

## 状态

已采纳，待实现。

## 概览

引擎仓库提供本地准备脚本，复用现有移动端构建流程，把 iOS 真机、iOS 模拟器和
Android 三类产物归集到仓库根的 `.artifacts/local/`，并生成 `local-prepared.json`，
供移动端本地开发按运行目标切换使用。

## 目标

- 一条命令产出三个平台目录与就绪清单，供移动端本地开发消费。
- iOS 真机与模拟器产物拆分为独立 XCFramework，本地构建只需当前目标切片。
- 现有发布构建行为保持不变。

## 非目标

- 不取代正式发布流程，不生成 HarmonyOS 产物。
- 移动端“切换本地产物”是产品仓动作，本说明只约定引擎侧交付物。

## 产物布局

```text
.artifacts/local/
  ios/       UniClipboardEngine.xcframework(.zip)、绑定、校验值、版本与提交
  ios-sim/   同上（模拟器切片）
  android/   UniClipboardEngine.aar、Kotlin 绑定、POM、校验值、版本与提交
  local-prepared.json
```

- `.artifacts/` 不纳入版本控制。
- iOS 构建脚本新增 `UC_ENGINE_UNIFFI_SLICE=device|simulator|universal`，
  默认 `universal` 保持现有行为；`device` 只含真机切片，`simulator` 只含模拟器切片。
- 脚本失败即止；每份主产物记录 sha256。

## local-prepared.json

最小稳定 schema：

```json
{
  "schemaVersion": 1,
  "engineVersion": "v1.0.0-rc.6",
  "sourceCommit": "4bf33a1",
  "preparedAt": "2026-08-08T00:00:00Z",
  "artifacts": [
    { "platform": "ios", "file": "ios/UniClipboardEngine.xcframework.zip", "sha256": "..." },
    { "platform": "ios-sim", "file": "ios-sim/UniClipboardEngine.xcframework.zip", "sha256": "..." },
    { "platform": "android", "file": "android/UniClipboardEngine.aar", "sha256": "..." }
  ]
}
```

- `engineVersion` 与 `sourceCommit` 取自同一构建提交，三平台必须一致。
- `file` 为相对 `.artifacts/local/` 的路径；消费方以 sha256 校验后使用。
