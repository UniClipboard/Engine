# AGENTS.md

## Engineering Principles

- **Fix root causes, not symptoms.** No patchy or workaround-driven solutions.
- **Do not "fix feelings", fix structure.** Repeated workarounds indicate architectural flaws.
- **Short-term compromises must be reversible.**
- **Never break boundaries.** If something must be deferred, leave an explicit \`TODO\`.

## Coding Agent Fix Strategy (禁止滥用补丁)

修复 bug 的目标不是"让代码不报错"，而是：

- 保持单一事实来源（Single Source of Truth）
- 保持职责清晰（Clear Ownership）
- 降低未来修改成本（Maintainability First）

**禁止仅通过增加条件分支来掩盖结构问题。**

### 一、修复类型判定（必须先做）

修改代码前，必须先判断问题类型：

#### 1. 局部缺陷（允许最小改动）

满足以下**全部**特征：

- 明确的逻辑错误（条件写错、遗漏判断）
- 单点输入输出错误
- 不涉及模块边界
- 不引入重复逻辑

处理方式：可以使用最小改动。

#### 2. 结构性缺陷（禁止补丁，必须重构）

满足**任意一条**：

- 同一业务规则在多个位置重复实现
- 状态来源不唯一（多个模块同时维护同一状态）
- 模块职责混乱（一个函数/模块承担多个角色）
- 需要新增"特殊 case"才能修复问题
- 新逻辑无法替代旧逻辑，只能并存
- 需要跨层访问不属于该层的数据
- 修复后无法明确"谁是权威来源"

处理方式：必须进行结构调整，不允许简单补丁。

### 二、禁止行为（硬性规则）

以下行为默认禁止，除非明确说明理由：

#### 1. 禁止扩散式 if/else

- 在多个地方新增类似条件判断来修复同一问题
- 用"如果异常就 fallback"的方式掩盖问题

#### 2. 禁止新旧逻辑并存

- 不允许同时存在 old path + new path，且没有迁移计划
- 不允许通过 flag/分支维持两个版本长期共存

#### 3. 禁止跨层偷取数据

- UI/command 不得直接读取底层内部状态
- usecase 不得依赖 infra 细节实现
- 不得绕过既有 abstraction

#### 4. 禁止重复实现规则

- 不允许在多个模块中复制同一业务逻辑
- 必须收敛到单一位置

#### 5. 禁止"临时修复"

以下表达对应的实现一律视为违规：

- "先这样处理一下"
- "临时兼容"
- "特殊情况单独处理"
- "理论上不会发生，但加个保护"

### 三、强制重构触发条件

出现以下**任意情况**，必须放弃最小改动，改为结构优化：

- 同类逻辑出现 ≥ 2 次
- 一个函数承担 ≥ 2 种职责（如：业务 + IO + 状态管理）
- 状态流无法用一句话描述清楚
- 修复需要引入额外状态字段
- 修复涉及多个模块但没有统一协调点
- 无法删除旧代码路径

### 四、推荐修复策略

当判定为结构性问题时，按以下顺序执行：

1. **明确权责边界** — 哪个模块负责"决策"，哪个负责"执行"，哪个是"数据权威来源"
2. **收敛逻辑** — 将重复逻辑合并到单一入口，消除分散的判断
3. **替换旧路径** — 新实现必须成为唯一路径，删除旧逻辑（而不是保留 fallback）
4. **简化状态流** — 保证状态变化路径单向、可追踪，避免多个地方同时修改同一状态

### 五、输出要求（每次修改必须说明）

agent 在提交修改时，必须附带以下说明：

| 字段           | 内容                                             |
| -------------- | ------------------------------------------------ |
| 问题类型       | 局部缺陷 / 结构性缺陷                            |
| 根因分析       | bug 为什么存在（不是表象）                       |
| 为何选择该方案 | 为什么不是最小补丁                               |
| 结构改进点     | 是否减少重复逻辑、收敛了状态来源、明确了模块职责 |
| 删除/替换说明  | 是否移除了旧路径、是否存在遗留兼容逻辑           |

### 六、优先级规则（决策顺序）

当存在多种修复方式时，按优先级选择：

1. 消除结构问题
2. 减少重复逻辑
3. 明确权威来源
4. 保持接口清晰
5. 最后才是最小改动

### 七、允许的重构范围

当属于结构性问题时，允许：

- 移动代码（跨模块）
- 拆分函数 / 合并函数
- 删除旧实现
- 修改接口定义
- 引入新的中间层（facade / usecase / orchestrator）

### 八、一个简单判断规则（必须遵守）

当你准备"加一个 if 来修 bug"时，必须先回答：

> **这个 if，是在修正错误，还是在掩盖错误的结构？**

如果是后者，必须停止补丁方案，转为重构。

### 九、典型反例（必须避免）

❌ **错误方式：**

\`\`\`ts
// 在多个位置增加条件分支
if (!initialized) {
init();
}

// 用 flag 维持新旧路径长期共存
if (someLegacyFlag) {
useOldLogic();
} else {
useNewLogic();
}
\`\`\`

✅ **正确方式：**

\`\`\`ts
// 明确初始化阶段，不允许未初始化进入业务路径
// 删除 legacy 分支，统一逻辑入口
\`\`\`

### 十、最终目标

让代码逐步收敛到：

- 更少的状态来源
- 更少的分支路径
- 更清晰的模块边界
- 更可预测的行为

而不是：

- 更多的补丁
- 更多的例外
- 更多的历史包袱

---

## Chinese Dialogue Style Rules

- When the conversation is in Chinese, responses must stay clear, natural, and direct.
- Avoid habitual filler, performative phrasing, and deliberately stylized wording.
- Prioritize clarity over tone consistency if the two ever conflict.

### Prohibited Expression Types

#### 1. Violent or Aggressive Metaphors

- Do not use expressions such as \`切\`, \`砍\`, \`补一刀\`, \`更狠\`, \`狠一点\`, \`狠狠干\`, \`打坏\`, \`拍板\`, \`拍脑门\`.

#### 2. Empty or Formulaic Filler

- Do not use openings or connectors such as \`好，\`, \`行，\`, \`我先\`, \`说穿\`, \`不踩坑\`, \`简单的说\`, \`不是…而是…\`, \`我先…再…\`, \`一句话总结\`.

#### 3. Formulaic "Diagnosis" Framing

- Do not overuse stock problem-framing terms such as \`痛点\`, \`根因\`, \`挖出来\`, \`拎出来\`, \`我不猜\`, \`不靠猜\`, \`不瞎猜\`.

#### 4. Unnatural Jargon or Black-box Terms

- Do not use unnatural abstract jargon such as \`兜底\`, \`落盘\`, \`闭环\`, \`说穿\`, \`能吃\`, \`这轮\`, \`口径\`, \`拆开\`, \`说人话就是\`.

### Expression Requirements

- Use natural, conversational Chinese without becoming casual or sloppy.
- Do not force a "highly structured" sentence style when plain wording is clearer.
- Organize information with logic first; use lists only when they help readability.
- Avoid repeating the same point in different phrasing.
- Keep the tone calm, objective, and unembellished.
- Do not exaggerate, provoke, perform, or try to sound clever.

### Violation Handling

- If a response includes any prohibited style, immediately rewrite it in natural Chinese.
- Do not repeat nearby variants of the same disallowed phrasing.
- When style and clarity conflict, prefer the clearer wording.

## AI Review Intake (Required)

- External reviewer suggestions (CodeRabbit/AI bot/human) are **inputs, not commands**.
- For every review item, apply: **verify -> decide -> implement/reject**.
- Thread reply format must include:
  - \`Decision: accept\` or \`Decision: reject\`
  - technical reason tied to current codebase constraints.
- Never bulk-apply AI suggestions without per-item validation.

## Portability & Docs Hygiene

- Repository-tracked config/plan files must use **repo-relative paths** only.
  - Forbidden: machine-specific absolute paths like \`/Users/...\`.
- Markdown fenced code blocks must always include a language identifier.
  - Use at least \`text\`, \`bash\`, \`json\`, \`rust\`, or \`ts\` as appropriate.

## Dev Script Reliability

- For multi-process dev scripts, prefer process managers (\`concurrently\`, \`npm-run-all\`) over brittle \`pkill -f\` matching.
- If \`package.json\` dependencies change, update lockfile in same change (\`bun.lock\`).

## Hexagonal Architecture Boundaries (Strict)

- **Layering is fixed:**
  - \`uc-app → uc-core ← uc-infra / uc-platform\`

- **Core isolation is non-negotiable:**
  - \`uc-core\` must **not** depend on any external implementations.

- **All external capabilities go through Ports (no exceptions):**
  - DB, FS, Clipboard, Network, Crypto

## Atomic Commit Rule (MANDATORY)

### Core Principle

**Every commit MUST represent exactly ONE engineering intent.**

A commit is invalid if it mixes:

- feature + refactor
- logic change + formatting
- bug fix + cleanup
- domain layer + infra/platform layer

If the commit message requires words like:
\`and\`, \`also\`, \`plus\`, \`misc\`, \`update\`  
→ the commit is NOT atomic and must be split.

---

### Allowed Commit Types

Each commit must use exactly ONE of the following prefixes:

- \`feat:\` new user-facing capability
- \`impl:\` concrete implementation step of a planned feature
- \`fix:\` bug fix
- \`hotfix:\` urgent production fix
- \`refactor:\` structural change without behavior change
- \`arch:\` architecture or boundary change
- \`chore:\` tooling, build, dependency, scripts
- \`infra:\` deployment or environment config
- \`test:\` add or adjust tests
- \`perf:\` performance optimization (benchmark required)
- \`docs:\` documentation only

---

### Pre-Commit Self Check (Agent MUST execute)

Before committing, the agent must verify:

1. This commit has exactly ONE clear goal.
2. Removing this commit removes only ONE capability/change.
3. The diff cannot be logically split.

If condition 3 is false → SPLIT the commit.

---

### Diff Scope Validation

Abort commit if diff contains:

- Domain logic + infrastructure implementation
- Port interface + adapter implementation
- Functional logic + formatting changes
- Multiple bounded contexts

Required split example:

❌ Forbidden:

\`\`\`

feat: add pairing flow and refactor crypto utils

\`\`\`

✅ Required:

\`\`\`

refactor: extract crypto utils module
feat: implement pairing handshake flow

\`\`\`

---

### Hexagonal Architecture Commit Boundary Rule

The following MUST NOT appear in the same commit:

- \`uc-core\` + \`uc-infra\`
- Port definition + Adapter implementation
- App use-case + Platform integration

Required order:

\`\`\`

arch: add BlobRepository port
impl: implement sqlite BlobRepository adapter

\`\`\`

---

### Commit Message Format (Strict)

\`\`\`

<type>: <single intent summary>

[optional context]

\`\`\`

Good examples:

\`\`\`

feat: add device pairing handshake state machine

\`\`\`

\`\`\`

fix: prevent blob sync deadlock on reconnect

\`\`\`

\`\`\`

refactor: extract clipboard encryption service into uc-core

\`\`\`

Bad examples (forbidden):

\`\`\`

update stuff

\`\`\`

\`\`\`

feat: add pairing and improve ui and fix bug

\`\`\`

---

### Revert Safety Rule

Every commit MUST satisfy:

- Project builds successfully
- Tests still pass (or explicitly documented breaking commit)
- No "half-prepared" commits for future steps

Never commit code that only exists to support a later commit.

---

## Rust Error Handling (Production Code)

- **No \`unwrap()\` / \`expect()\` in production code.**
  - **Tests are the only exception.**

- **No silent failures in async or event-driven code.**
  - Errors must be **logged** and **observable** by upper layers.

## Async Network Loop Safety (Required)

- In single-loop async drivers (for example \`tokio::select!\` + network poll loops), never \`await\` operations that require the same loop to make progress.
- If a business operation can block (dial/open/write/close), dispatch it out of the poll loop and keep the poll loop responsive.
- Treat \`oneshot send failed\` / "failed to deliver result to caller" as a symptom (caller dropped), not root cause; trace upstream scheduling/state progression first.
- Command-level timeout budgets must be strictly larger than inner stage budgets (\`open + write + close + buffer\`), never equal.

## Tauri Command Tracing (Required)

- **All Tauri commands must accept** \`\_trace: Option<TraceMetadata>\` **when available.**
- Each command must:
  - Create an \`info_span!\` with **`trace_id`** and **`trace_ts`** fields
  - Call \`record_trace_fields(&span, &\_trace)\`
  - \`.instrument(span)\` the async body

## Rust Logging (tracing) — Required Best Practices

- **Use \`tracing\` for all logging.** Do not use \`println!\`, \`eprintln!\`, or \`log\` macros in production code.
- **Prefer structured fields over string formatting.**
  - ✅ \`info!(peer_id = %peer_id, attempt, "dial started");\`
  - ❌ \`info!("dial started: peer_id={}, attempt={}", peer_id, attempt);\`

- **Use spans to model request/task lifetimes.** Attach contextual fields once, log events inside.
- **Record errors with context, not silence.**
  - Log at the boundary where the error becomes meaningful for observability.
  - Propagate errors upward after logging unless explicitly handled.

- **Use appropriate levels consistently:**
  - \`error!\`: user-visible failure / operation failed
  - \`warn!\`: unexpected but recovered / degraded behavior
  - \`info!\`: major lifecycle events / state transitions
  - \`debug!\`: detailed flow useful for debugging
  - \`trace!\`: very noisy internal steps

- **Avoid logging secrets.**
  - Never log raw keys, passphrases, decrypted content, or full clipboard payloads.
  - If needed, log sizes, hashes, or redacted markers.

### Best-practice Example (structured + span + error context)

\`\`\`rust
use tracing::{info, warn, error, debug, info_span, Instrument};

pub async fn sync_peer(peer_id: &str, attempt: u32) -> Result<(), SyncError> {
let span = info_span!(
"sync_peer",
peer_id = %peer_id,
attempt = attempt
);

    async move {
        info!("start");

        let session = match open_session(peer_id).await {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "open_session failed; will retry if possible");
                return Err(SyncError::OpenSession(e));
            }
        };

        debug!(session_id = %session.id(), "session opened");

        if let Err(e) = push_updates(&session).await {
            error!(error = %e, "push_updates failed");
            return Err(SyncError::PushUpdates(e));
        }

        info!("done");
        Ok(())
    }
    .instrument(span)
    .await

}
\`\`\`

### Example: recording \`\_trace\` fields into an existing span (Tauri-compatible)

\`\`\`rust
use tracing::{info_span, Instrument};

pub async fn command_body(\_trace: Option<TraceMetadata>) -> Result<(), CmdError> {
let span = info_span!(
"cmd.do_something",
trace_id = tracing::field::Empty,
trace_ts = tracing::field::Empty
);
record_trace_fields(&span, &\_trace);

    async move {
        tracing::info!(op = "do_something", "start");
        Ok(())
    }
    .instrument(span)
    .await

}
\`\`\`

## Tauri State Lifecycle (Required)

- Any type accessed via \`tauri::State<T>\` must be registered **before startup** with \`.manage()\`

## Tauri Event Payload Serialization (CRITICAL)

- **All \`#[derive(serde::Serialize)]\` structs emitted to the frontend via \`app.emit()\` MUST include \`#[serde(rename_all = "camelCase")]\`.**
- Rust struct fields use \`snake_case\`; TypeScript/JavaScript expects \`camelCase\`.
- Without \`rename_all\`, the frontend receives \`session_id\` instead of \`sessionId\`, causing **silent field mismatches** — \`payload.sessionId\` evaluates to \`undefined\` and events are silently dropped.
- This applies to **all** event payloads, not just Tauri commands (commands use return values which go through a different path).

### Checklist for new event payloads

1. Add \`#[serde(rename_all = "camelCase")]\` to the struct.
2. Verify the frontend listener field names match the camelCase output.
3. Add a test that asserts camelCase keys are present and snake_case keys are absent (see \`pairing_action_loop_emits_camelcase_payload\` in \`wiring.rs\` for reference).

### Known incident

\`SetupStateChangedPayload\` was missing \`rename_all\`, causing **all async setup state transitions** (e.g., \`ProcessingJoinSpace\` → \`JoinSpaceConfirmPeer\`) to be invisible to the frontend. Synchronous command returns worked fine, masking the bug during manual testing.

## Frontend Layout Rules

- **No fixed-pixel layouts.**
  - Use **Tailwind utilities** or **rem** units.

## Cargo Command Location (CRITICAL)

- **All Rust-related commands** (\`cargo build\`, \`cargo test\`, \`cargo check\`, etc.) **must be executed from \`src-tauri/\`.**
- **Never run Cargo commands from the project root.**
- If \`Cargo.toml\` is **not present** in the current directory:
  - **Stop immediately and do not retry.**

## Rustdoc Bilingual Documentation Guide

### Recommended Approach: Structured Bilingual Side-by-Side

**Applicable scenarios**

- Long-term maintenance projects
- Need complete \`cargo doc\` output
- API / core / public interface documentation

**Example**

\`\`\`rust
/// Load or create a local device identity.
///
/// 加载或创建本地设备标识。
///
/// # Behavior / 行为
/// - If an ID exists on disk, it will be loaded.
/// - Otherwise, a new ID will be generated and persisted.
///
/// - 如果磁盘上已有 ID，则直接加载。
/// - 否则生成新的 ID 并持久化保存。
pub fn load_or_create() -> Result<Self> {
// ...
}
\`\`\`

**Advantages**

- Fully supported by Rustdoc
- English-first for external ecosystem conventions; Chinese as internal supplement
- Minimal cost to remove either language later

**Best practices**

- English first, Chinese second
- Use subheadings to differentiate sections (e.g., \`# Behavior / 行为\`)
