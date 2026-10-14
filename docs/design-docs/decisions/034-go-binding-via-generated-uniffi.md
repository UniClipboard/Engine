# ADR-034：Go 绑定使用生成的 UniFFI 包与薄 Go 门面

- **状态**：Accepted（第一片：macOS arm64 实测；其余平台按能力覆盖）
- **相关文档**：[执行计划](../../exec-plans/active/2026-10-07-go-binding.md)、[`bindings/go/README.md`](../../../bindings/go/README.md)、[uc-engine 接口](../uc-engine-interface.md)

## 背景

后续 Go daemon 需要在同一进程内托管 Rust Engine。现有 UniFFI 绑定只服务 Swift 与 Kotlin。重写 Iroh、加密或持久化不在考虑内。

## 决策

- 复用现有 UniFFI（`=0.31.1`）元数据，用 NordSecurity `uniffi-bindgen-go` 固定 revision 生成 Go/cgo 包；生成器自身 `Cargo.lock` 与模板补丁的 sha256 固定在 `bindings/go/generator/PIN.env`，不进入本仓 workspace 和 `Cargo.lock`。
- 上游模板的局部变量与用户方法参数同名时生成物无法编译；修复只通过生成器源码补丁交付，禁止手改生成文件。生成物提交入库，CI 重新生成要求零差异。
- Go 门面只做 Go 侧所有权：一次 `Open`、调用 `uc-engine` 既有动作、读取事件、确定性 `Close`、稳定错误与输入校验。业务流程、恢复与重试仍由 Rust 负责。
- 原生库不进入 Go module。`stage-native.sh` 为库生成来源清单（Engine revision、`Cargo.lock` sha256、目标、profile、工具链、生成器 pin、库 sha256/size），`Open` 强制核对清单与动态链接器报告的库文件（完整性自检，不防御有本地写权限的攻击者），没有跳过模式。
- 第一片不修改 Rust 源码与公共 UniFFI 表面，因此 Swift/Kotlin 契约与移动 pin 不受影响。

## 取舍与代价

- 嵌入后 Rust 崩溃（`panic=abort`、越界、OOM）会终止整个宿主进程，Go 的 `recover` 无效，失去现有 daemon 与 GUI/CLI 之间的进程隔离。是否为 Go daemon 采用嵌入模式、
  GUI/CLI 是否仍经 HTTP 连接独立 daemon，须在 daemon 重写评审中明确决定，不默认嵌入更优。
- 生成器仍是 0.x 第三方项目，约半年无新提交；固定 revision 与补丁降低漂移，但维护来源（fork 或上游贡献）尚待决定。
- 动态库需要宿主打包与签名；Windows 的导入库与库身份校验尚无方案。

## 被放弃的方案

- 手写窄 C ABI：重复 UniFFI 已有的对象、错误与回调机制，维护面更大。仅在生成器出现无法解决的缺口时作为备选。
- 手改生成后的 Go 文件：重新生成即丢失，无法重复。
- 在 Go 门面再设事件缓冲：与 Rust 已有的有限队列和 `RefreshRequired` 形成两层背压。
