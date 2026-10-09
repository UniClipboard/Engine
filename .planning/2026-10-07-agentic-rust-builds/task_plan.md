# MBX 统一 Rust 构建入口

## 当前范围
统一本地仓库脚本、常规 CI、端到端与发布构建的 Cargo 调用使用固定 MBX，删除活动 sccache 路线；不改 main 保护、不合并、不索取密钥。

## 验收契约
完整负责人 scripts/build-cache/mbx.sh 与仓库 Cargo 入口。调用方执行原测试/构建脚本或 just cargo；CI setup 安装固定工具并配置 job 专属入口。返回真实命令退出码，失败/重试责任留在调用方，缓存错误不伪造命中。首次构建、不可缓存动作、metadata/fmt/测试执行没有可声称的 rustc 命中。R2 写入仍由 native protected-push 与桶令牌双重约束。

## 实施前失败方式
- cargo shim 递归、自身 PATH 遮蔽 real Cargo；嵌套 ndk/nextest/llvm-cov 子进程绕过入口。
- 测试执行被跳过或被缓存结果替代；恢复二进制 provenance 错误。
- 升级锚点 checkout 不含新入口，隔离 source 继承错误路径。
- CI secrets 写到日志/GITHUB_ENV；PR/手工/tag 持有 writer 或 main 未保护误记发布。
- fork 无凭据，远端不可达；只读空 namespace 无命中。
- release target/profile/路径重映射/signature 被缓存流程改动。
- 动作缓存与 target 被不当清理；旧失败证据丢失/library 超限。
- 节点单元守护依赖旧 sccache 设计，须在实施前按新契约更换为实际 E2E 证据。

## 阶段
1. 盘点入口、官方 native 行为、既有依赖：完成。
2. 统一入口并完成最小测试编译/恢复/执行 E2E：完成，653 hits / 1747 restored，2 tests passed。
3. 扩展 CI、移动/发布脚本、删除旧实现、更新 ADR：已实施，开始验证。
4. 串行完整验证、真实缓存命中证据、commit/push/CI 跟进。
5. 小型 library 与完整 artifacts 交接及 home 同步。
