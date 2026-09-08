# 旧 UCBL 密文不可读时的资料升级修复

## Goal

旧 UCBL 的 AEAD 认证失败不再中止整个 V3 资料升级：原密文保留，全部引用标为 Lost，可读数据继续转换。文件读取、格式、会话/密钥解析和目标完整性错误仍失败关闭。

## Next Step

已完成真实 dev 资料升级及重复启动；由用户继续界面手动验收。

## Current Phase

Phase 6

## 负责人和结果

- 完整负责人：Infra 的 ProfileStorageUpgrade。
- 调用方唯一动作：ensure_v3()，既有返回合同不变。
- 成功：完成可读数据转换，保留旧密文与不可用引用，通过原子发布及验证。
- 失败：保留介质、格式、会话、密钥解析及其他转换错误的 source chain，不发布半转换结果。
- 重启/重试：继续使用既有加密 journal 和 generation；清理来源后保留副本仍在活动 blob tree。

## Phases

### Phase 1: 复现
- [x] 核对前序现场记录与现有生产代码。
- [x] 运行真实转换测试，确认旧 AEAD 失败中止转换。
- **Status:** complete

### Phase 2: 边界
- [x] 确认原密文保留位置、Lost 状态及错误分类。
- [x] 保持正常 V3 reader 和调用方接口不变。
- **Status:** complete

### Phase 3: 实现
- [x] 共用唯一旧格式解码器，区分读取失败与 AEAD 失败。
- [x] 原子保留密文和更新引用，校验字节、状态与目录摘要。
- [x] 同步架构圣经稳定规则和维护记录。
- **Status:** complete

### Phase 4: 验证
- [x] V1 错误旧密钥、缺文件 source chain、未知格式、失败重试、状态/文件篡改回归。
- [x] 真实生产升级、多个引用、孤立 blob、nullable 算法、来源清理、重启与明文探针。
- [x] 安全相关库测试 148 项及完整升级集成测试 15 项通过。
- [x] metadata、workspace all-targets check、fmt、架构门禁和 diff check 通过。
- 原始桌面 profile：**跳过**，本会话未提供资料位置与密钥环境；前序现场统计不充作本轮验证。
- 实体设备与发布矩阵：**跳过**，本轮未执行。
- **Status:** complete

### Phase 5: 交付
- [x] 复核最终修改与错误边界，记录验证和跳过项。
- [x] 保留其他依赖审计计划和活动指针，不提交或发布。
- **Status:** complete

## 错误记录

完整过程见 progress.md。测试命令首次短名称与 --exact 不匹配已修正；真实数据库测试发现 nullable 算法列、唯一 hash 约束及空 V2 fixture 缺少持久群组材料，均已按真实合同修正并通过验证。

### Phase 6: 手动测试后的新阻塞
- [x] 确认 desktop 的 dev daemon 已用新构建，升级失败从 corrupt 变为 security。
- [x] 定位真实 profile 新错误，补回归并修复。
- [x] 重建 desktop daemon 并验证启动：首次与重启均 HTTP 200 / ok；旧来源清理后加密快照仍在。
- **Status:** complete
