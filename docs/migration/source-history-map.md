# 源码迁移与历史映射

## 迁移基线

- 来源仓库：`UniClipboard/UniClipboard`
- 来源分支：`refactor/unified-core-independent-crates`
- 来源提交：`12104cbab7a3b167f33f95c5a9d6d7d90fbbfa75`
- 过滤工具：`git-filter-repo 2.47.0`，工具提交 `a40bce548d2c`
- 过滤后对应提交：`f7212ec69e79dea0ed2949288cc3642f493cf210`

完整提交映射保存在本地过滤记录 `.git/filter-repo/commit-map` 中；该目录不是项目内容，不随仓库发布。上面的切换基线是跨仓审计的稳定锚点。

## 路径映射

| 来源仓库路径 | 本仓路径 |
| --- | --- |
| `crates/uc-engine-uniffi` | `bindings/uc-engine-uniffi` |
| `crates/uc-ohos-napi` | `bindings/uc-ohos-napi` |
| `crates/uc-mobile-proto` | `compatibility/uc-mobile-proto` |
| `crates/uc-mobile` | `compatibility/uc-mobile` |
| `apps/mobile-probe-core` | `tests/hosts/uc-mobile-probe-core` |
| `apps/android-probe` | `tests/hosts/android` |
| `apps/ios-probe` | `tests/hosts/ios` |
| `apps/ohos-probe` | `tests/hosts/ohos` |

## 所有权切换

在消费者切换完成前，本仓使用受保护的迁移分支，不创建 `core-v*` 标签。切换后，核心行为、协议、持久化、数据库迁移和绑定修改只进入本仓；产品仓只保留平台接入和固定版本记录。
