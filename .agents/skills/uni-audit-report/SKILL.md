---
name: uni-audit-report
description: "汇总多个独立 Uni 维护审查 job 产生的 JSON artifacts，校验覆盖、合并重复 finding、保留证据与跳过项并生成统一报告。用于 GitHub Actions 并行审查完成后的独立汇总 session；不读取源码重新审查，不实现修复，也不调用其他 skill。"
---

# Uni 审查结果汇总

## 目的

把并行 job 的独立报告转成一份可行动、可追溯的总报告。汇总只消费输入 artifacts，不重新解释源码，不补做缺失 lane，也不把缺失报告当作通过。

## 独立运行约束

- 一个 session 只运行本 skill，不调用或模拟任何专项审查 skill。
- 完整读取 [finding contract](references/finding-contract.md)。
- 输入仅来自调用方明确提供的 artifact 文件或目录；不要在仓库源码中寻找新证据。
- 除调用方指定的汇总输出路径外，不修改仓库、Issue 或 PR。

## 工作流

1. 读取调用方指定的预期 lane 清单、运行标识和 artifact 路径。没有显式清单时，预期五路：`module_depth`、`failure_recovery`、`runtime_resilience`、`observability`、`security_contract`。
2. 校验每个 artifact 的 JSON、`schema_version`、lane 唯一性、scope、status、checks、findings 和 redaction 声明。
3. 对缺失、重复、版本不兼容或无法解析的 artifact 建立 coverage 问题；不得伪造 lane 内容。
4. 先按 `dedupe_key` 合并完全相同的根因，再审阅标题相近但 key 不同的项。只有根因、owner 和失败模式相同才能合并。
5. 合并时保留所有来源 lane 和证据：
   - 严重度采用证据能够支持的最高级别，不因多路重复而升级；
   - 置信度保留各路差异，不用平均值掩盖不确定性；
   - 任一路提供有效 `debt_ref` 时标明已声明债务，但仍保留其他 lane 的新增证据；
   - 相互矛盾的结论并列展示并标为需要人工裁决。
6. 生成 answer-first 总结：覆盖状态、最高风险、按严重度排序的去重 finding、已有债务、疑似项、lane handoff、检查结果和 skipped 项。
7. 若调用方指定 JSON 和 Markdown 输出路径，分别写入机器可读汇总与人类报告；否则在最终回复返回 Markdown。

## 禁止事项

- 不读取源码来“修好”缺失证据。
- 不执行 cargo、测试、架构检查或发布检查。
- 不调用其他 skill 或启动额外审查 session。
- 不把 `no findings` 改写为“健康”或“通过”。
- 不回显 artifact 中疑似敏感的原始文本；发现未脱敏内容时只报告 artifact、字段路径和违规类型。

## 完成条件

- 每个预期 lane 明确标为 `completed`、`partial`、`blocked` 或 `missing`。
- 去重后仍能追溯到原始 lane、finding id 和证据位置。
- 总数可由去重 finding 重新计算，且 skipped 项没有被算作 pass。
- 报告明确声明它只汇总输入 artifacts，没有重新审查源码。
