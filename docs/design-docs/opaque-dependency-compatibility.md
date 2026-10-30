# OPAQUE 密码依赖兼容来源

## 责任与边界

`uc-infra-crypto/space_admission_auth` 继续负责完整密码运算及算法资料格式；Application 的准入负责人继续拥有握手、失败与重试责任，宿主沿用原调用。依赖兼容修复不增加业务入口、后台重试或资料迁移：旧资料解密后仍由相同解码器读取；错误密码、损坏消息与上下文不匹配仍拒绝。密码材料不落普通文件或日志。

本次额外范围由用户明确批准，服务于第八片依赖审计阻塞；它不代替阶段0物理挂起证明，也不授予生产进度迁移资格。

## 为什么维护受限来源

[RUSTSEC-2026-0387](https://rustsec.org/advisories/RUSTSEC-2026-0387.html) 修复要求为 `crypto-bigint >=0.7.6`。原 `opaque-ke 4.0.1 → voprf 0.5.0／elliptic-curve 0.13.8 → crypto-bigint 0.5.5` 引入受影响实现。关闭默认特性不能移除必需依赖；当前上游预发布也仍依赖该链。不能仅凭使用Ristretto套件就忽略审计，也不修改审计规则。

本产品实际套件保持 `Ristretto255 / SHA-512 / TripleDh / Argon2id`。第三方兼容来源位于 [`third_party/opaque-ke`](../../third_party/opaque-ke/) 和 [`third_party/voprf`](../../third_party/voprf/)，分别基于 crates.io `4.0.1` 和 `0.5.0`，版本构建标识为 `+uni.1`，禁止发布。固定来源和局部变化由各自 `UPSTREAM.json` 及 `UPSTREAM.patch` 记录；`node scripts/architecture/check-opaque-dependency-source.mjs` 核对源码、许可、清单和明确的修改边界。原版源码已按公开crate归档校验和逐文件核对。

兼容来源移除未使用的通用短Weierstrass曲线实现、相应blanket impl及ECDSA后端，从解析图中真正移除旧EC／大整数实现；没有重命名漏洞包或添加ignore。Ristretto仅需原EC库中的RFC9380消息展开，故保留 `elliptic-curve 0.13.8` 的 `expand_msg` 源码及向量，放入VOPRF私有模块：只改模块路径和局部无信息错误类型，算法不变。原许可随源码保留；不复制或修改大整数算术。OPAQUE显式启用 `generic-array/zeroize`，保留原先由EC依赖间接开启的清零能力。

真实path依赖逐级写入Infra和OPAQUE，确保独立Git消费者得到同一来源；不用根 `[patch]`，产品仓无需补丁。通用EC与ECDSA不再是本兼容来源提供的能力；Engine稳定接口不暴露这些库类型。

## 必需验证与复跑

- `node scripts/testing/run-opaque-dependency-compat.mjs`：从固定旧Engine提交 `11470feee1cc5d6e8e4b77b6b1f12ddaedeff628` 归档源码，注入相同测试，先构建并复制旧二进制，再构建候选。旧旧基线通过后，分别以新旧客户端／服务端读取新旧来源资料完成握手，核对资料重编码逐字节相同及双方继续凭据一致；双向覆盖错误密码和损坏消息。测试资料仅经私有子进程管道交换。原始编译输出、源码／lock／测试／二进制指纹默认写入 `target/test-reports/opaque-compat/`；临时源码和二进制在结束时回收，共用仓库target。
- `just cargo test -p uc-infra-crypto -p uc-infra-profile --test opaque_expand_message --test opaque_rfc9807 --test space_admission_auth --locked`：执行原消息展开向量、RFC9807精确握手向量及原有密码／损坏输入／上下文边界。
- Infra crypto／profile完整测试验证已有加密资料与存储集成；`node scripts/architecture/check-engine-git-consumer.mjs` 必须证明OPAQUE和VOPRF也来自同一Engine Git来源。
- `just cargo audit --deny unsound` 及根门禁必须通过；移动编译由实际候选CI证明，未执行的平台保持跳过。

兼容runner由PR Check强制执行；普通workspace测试保留两个明确ignored的专用IPC入口，不能据默认测试通过声称混合版本已测。

## 维护与退出

维护方负责审查原版到本地源码的局部差异、许可和来源清单，升级时重新跑以上固定旧端、精确字节与真实资料矩阵。上游提供能移除受影响依赖、保持本产品套件及资料兼容的版本后，优先回归上游；不得长期维护两套生产实现，删除兼容源码前须验证独立Git消费者和旧资料。

本记录说明稳定维护边界；本候选实际通过范围及未执行项只在[第八片可行性记录](../exec-plans/completed/2026-10-24-transfer-progress-connectivity-feasibility.md#阶段-0-结论)维护。
