# 六位匹配码本地验收

日期：2026-09-07。状态：本地流程验收完成，实体设备和生产交付跳过。

## 范围与责任

接手时 Infra 已有六位生成及 `codeLength: 6` 请求改动；本次保留它们，补齐前导零请求检查和实际短码加入验收。
Application 继续负责完整准入与恢复，调用方仍只执行 `IssueInvitation` 和 `JoinSpace`；成功形成目标空间的活动成员，失败沿既有结果返回，重启与重试责任不变。

现有成员测试通过完整邀请加入，不能证明短码可用。因此新增独立测试模块，直接提交邀请码，并核对两个 Engine 最终均有两名活动成员。
测试目录严格匹配 `000-001`，不以任意已登记邀请兜底；HTTP client 检查创建响应、解析和消费均保留前导零与横线。

## 已运行

从 Engine 根目录串行执行，复用仓库共享 `target`：

```bash
cargo test -p uc-infra --lib rendezvous:: --locked
cargo test -p uc-infra --lib pairing:: --locked
cargo test -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e six_digit_pairing:: --locked -- --test-threads=1
UC_SIX_DIGIT_RENDEZVOUS_URL=http://127.0.0.1:18789 cargo test -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e six_digit_pairing::local_rendezvous_joins_and_switches_using_six_digit_codes --locked -- --ignored --exact
```

- Rendezvous 29 项、Pairing 8 项通过。
- 常规短码测试 3 项通过：前导零新设备加入、前导零已有空间切换、目录不可达时本地生成及 mDNS 加入。
- 需本地服务的测试在常规运行中跳过，随后显式运行通过，实际完成新设备加入和已有空间切换两条流程。
- 本地服务使用相邻 `uc-rendezvous` 仓库提交 `3e04eb45f4d9aa129e11667f51d36cbb2e78ffc0`，运行 `wrangler dev --local --ip 127.0.0.1 --port 18789 --inspector-port 0 --log-level error`；验收后已停止。
- 服务端原有 `npm test` 共 35 项通过，包含真实本地存储的创建、解析和消费。
- 额外真实 HTTP 请求验证 `000-001`、漏横线查找失败、消费后拒绝解析、重复消费拒绝及过期失效。过期现有响应是 HTTP 404 / `pairing_expired`，初次探针误期望 410；核对服务源码后修正预期并重跑通过。
- `cargo metadata --locked --format-version 1`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、`node scripts/architecture/check-engine-repository.mjs` 和 `git diff --check` 通过；编译检查有既有无用代码提示。

## 验收边界

- 以上是本机双 Engine 实例的真实连接与准入，不是两台实体设备。
- 显式仅局域网模式：跳过。现有多实例测试构建固定关闭进程级仅局域网标志；目录不可达测试只证明本地生成及局域网发现路径，不能代替该设置的实机验收。
- 生产部署、两台实体设备和桌面发布构建：跳过。未修改桌面正式依赖，也未创建发布或提交。
- 本次基于 Engine `0f3a5843f81e1193ab524128ff4de292ac18220d` 加工作区改动验证；该提交本身不包含六位码改动，不可作为桌面升级目标。
