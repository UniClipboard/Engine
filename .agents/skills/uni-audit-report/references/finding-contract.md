# 维护审查 Finding Contract

本文件是五个专项审查 skill 与结果汇总 skill 的唯一输出契约。专项 job 可以并行运行，但每个 session 只运行一个 skill；任何专项 skill 都不得调用其他 skill。

## 执行边界

- 审查默认只读。只有调用方明确给出输出路径时，才写报告 artifact。
- 不自动修改生产代码、文档、测试、Issue、PR 或远端状态。
- 不清理或覆盖已有工作树修改。
- 搜索结果、数量、命名和静态模式只是候选信号；finding 必须有可达路径和规则证据。

## Scope

每次运行必须选择并记录一种模式：

- `diff`：审查调用方给出的 base/head；本地未给 ref 时只审查 staged、unstaged 与 untracked 的当前工作树，并将 `base_ref` 记为 `working-tree`。
- `targeted`：审查调用方给出的路径、symbol 或流程，可为理解调用链读取相邻代码。
- `full`：审查本 lane 涉及的整个仓库。定时审查必须由调度方显式选择 `full`，不得通过“最近七天”等模糊时间范围替代。

报告必须列出实际覆盖路径、排除项和用于确定 diff 的 ref/SHA。范围无法确定时返回 `blocked`，不要自行猜测。

## 证据类别

- `confirmed`：当前代码存在可达路径，且有实现位置、规则依据和明确影响。
- `suspected`：信号可信但仍缺少运行时、平台或完整调用路径证据；必须写出需要的验证。
- `existing_declared_debt`：同一根因已在 active plan 或技术债追踪器中明确记录，并能提供 `debt_ref`。

无 finding 时使用空数组；这只表示“在所述范围和证据下未发现”，不等于系统通过或健康。

## 严重度

- `critical`：可导致敏感数据/密钥暴露、不可恢复的数据破坏、信任或发布完整性绕过，且路径直接可达。
- `high`：很可能造成严重用户影响、广泛不可用、错误恢复或稳定契约破坏。
- `medium`：有具体路径的诊断盲区、局部可靠性风险或显著维护负担。
- `low`：影响有限、局部且有现成缓解措施，但仍违反明确规则。

严重度描述影响，不描述修复工作量。疑似项不能仅因最坏假设而提高严重度。

## 置信度

- `high`：从入口到影响的路径和规则均已直接验证。
- `medium`：主要路径已验证，但缺少一个运行时或平台证据。
- `low`：合理信号，仍需关键验证；通常应使用 `suspected`。

## 脱敏

报告不得包含剪贴板内容、密码、密钥、完整令牌、邀请、设备名、网络地址、运行时文件名或路径。不要复制原始错误文本、payload、数据库值或 fixture 秘密。

源码位置使用仓库相对路径和行号，这是代码证据，不是运行时业务路径。敏感问题只描述数据类别、来源、变换和汇点。

## JSON Artifact

专项 skill 在调用方指定路径写一个 UTF-8 JSON 对象；没有指定路径时，在最终回复中呈现同等信息。字段如下：

```json
{
  "schema_version": 1,
  "run_id": "由调度方提供；缺省时使用 head SHA",
  "lane": "module_depth | failure_recovery | runtime_resilience | observability | security_contract",
  "status": "completed | partial | blocked",
  "scope": {
    "mode": "diff | targeted | full",
    "base_ref": "可空",
    "head_ref": "可空",
    "paths_reviewed": [],
    "excluded": []
  },
  "sources_of_truth": ["仓库相对路径#章节"],
  "checks": [
    {
      "name": "检查名称",
      "status": "pass | fail | skipped",
      "detail": "不含敏感值的简短事实"
    }
  ],
  "findings": [
    {
      "id": "lane 内稳定且唯一的短标识",
      "dedupe_key": "owner|failure-mode|primary-symbol",
      "classification": "confirmed | suspected | existing_declared_debt",
      "severity": "critical | high | medium | low",
      "confidence": "high | medium | low",
      "title": "以影响为中心的标题",
      "rule": {
        "path": "规则文档相对路径",
        "section": "章节",
        "summary": "被违反规则的转述"
      },
      "evidence": [
        {
          "path": "源码相对路径",
          "line": 1,
          "symbol": "可空",
          "observation": "不含敏感值的事实"
        }
      ],
      "impact": "对调用者、用户、数据或运维的具体影响",
      "owner": "应承担完整责任的模块或 seam",
      "verification": "确认或否定该问题所需的最小验证",
      "gate_gap": "现有自动检查为何未覆盖；没有则为 null",
      "debt_ref": "已有债务的仓库相对路径；否则为 null"
    }
  ],
  "handoff": [
    {
      "suggested_lane": "其他 lane",
      "location": "源码相对位置",
      "reason": "为什么值得由该 lane 独立审查"
    }
  ],
  "redaction": {
    "reviewed": true,
    "notes": "脱敏说明"
  }
}
```

## 输出质量门槛

- Finding 按严重度、置信度、稳定 id 排序。
- 一条 finding 至少有一个源码证据和一个规则依据；纯覆盖缺失写在 scope/checks，不伪装成源码问题。
- 同一根因在本 lane 内只报告一次，多处证据放入同一 finding。
- 已声明债务必须给出精确 `debt_ref`；无法确认时用 `suspected`。
- 检查未运行或设备/产物不可用时必须是 `skipped`，不得记为 `pass`。
- `status=completed` 只表示本 lane 按声明范围完成审查，不表示没有风险。
- 写出报告前再次执行脱敏检查，并确认 JSON 可解析、计数可由 `findings` 推导。
