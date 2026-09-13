# 性能套件

性能套件只使用合成资料，默认不读取真实 profile。快速检查：

```bash
node scripts/performance/run.mjs --suite smoke
```

完整准入仓储规模曲线：

```bash
node scripts/performance/run.mjs --suite admission_repository
```

成员更新积压的稳定读取规模曲线：

```bash
node scripts/performance/run.mjs --suite group_update_delivery
```

每次运行把来源、环境和结果状态写入 `target/performance/`，Criterion 的原始统计保留在默认 `target/criterion/`。同一机器、工具链和构建配置下比较结果；工作区不干净会记录在 manifest 中，不能作为冻结发布基线。

准入场景测量“没有当前加入，但仓储含无关大记录”时的首次查询。每次样本重新创建仓储对象，清空应用内缓存；不宣称清除了操作系统磁盘缓存。

成员更新场景先完成一次旧资料整理，再测量反复读取八条到期任务；空间安全材料覆盖 0、1、7、25 MiB。整理成本不计入稳定读取结果，因此该场景只用于证明后台重试不再重复读取大资料，不能代表旧资料首次升级耗时。
