# 文档索引

本文档库按信息用途组织，避免把需求、稳定行为和重大取舍混在同一份文件中。

- [架构总览](architecture/architecture-bible.md)：当前系统边界、职责和维护规则。
- [需求](prd/)：要解决的问题、范围和验收标准。
- [技术说明](specs/)：稳定行为、接口和运行约束。
- [决策记录](adr/)：重要技术取舍及其后果。

## 需求

- [PRD-001：本地加密历史搜索](prd/001-local-encrypted-search.md)

## 技术说明

- [本地加密搜索](specs/001-local-encrypted-search.md)
- [离线优先成员移除](specs/015-offline-first-member-removal.md)
- [工作空间全局收敛](specs/016-workspace-wide-convergence.md)
- [配对作为工作空间准入通道](specs/017-pairing-as-workspace-admission.md)
- [按设备精确公开收敛等待状态](specs/019-device-specific-convergence-waiting-status.md)
- [个人设备信任核对产品契约](specs/021-device-trust-reconciliation-product-contract.md)
- [当前成员运行范围统一派生](specs/022-current-member-runtime-scope.md)
- [可持续验证的成员历史与准入激活](specs/023-durable-membership-proof-and-admission-activation.md)
- [成员收敛内部职责边界](specs/024-workspace-convergence-internal-boundaries.md)
- [用户明确加入安全取代旧加入](specs/025-user-initiated-join-supersession.md)
- [旧资料独立化与重新配对](specs/026-legacy-profile-isolation-and-re-pairing.md)
- [Engine 仓库检查](specs/engine-repository-checks.md)
- [Port 定义](specs/ports.md)
- [uc-engine 跨平台核心接口](specs/uc-engine-interface.md)

## 决策记录

- [ADR-002：剪贴板详情资源协议](adr/002-clipboard-resource-protocol.md)
- [ADR-003：缩略图资源协议](adr/003-thumbnail-resource-protocol.md)
- [ADR-004：剪贴板恢复单一表示](adr/004-restore-single-representation.md)
- [ADR-005：抽取 uc-engine](adr/005-uc-engine-extraction.md)
- [ADR-009：文件传输 Port 拆分](adr/009-file-transfer-port-split.md)
- [ADR-010：目录同步文件集清单](adr/010-directory-sync-as-file-set-manifest.md)
- [ADR-011：可靠成员撤销](adr/011-reliable-member-revocation.md)
- [ADR-012：主动刷新共享设备](adr/012-automatic-shared-device-refresh.md)
- [ADR-013：本地加密索引](adr/013-local-encrypted-search-index.md)
- [ADR-014：应用层使用动态 Port](adr/014-dynamic-ports-in-use-cases.md)
- [ADR-015：离线优先成员移除](adr/015-offline-first-member-removal.md)
- [ADR-016：工作空间全局收敛](adr/016-workspace-wide-convergence.md)
- [ADR-017：配对作为工作空间内部的准入通道](adr/017-pairing-as-workspace-admission.md)
- [ADR-018：应用层按业务领域收口](adr/018-domain-oriented-application-layout.md)
- [ADR-019：工作空间收敛等待状态按设备精确公开](adr/019-device-specific-convergence-waiting-status.md)
- [ADR-020：设备上线核对成员历史，未确认的移除由用户决定](adr/020-membership-reconciliation-and-user-decisions.md)
- [ADR-021：成员收敛的内部职责边界](adr/021-workspace-convergence-internal-boundaries.md)
- [ADR-022：用户明确加入创建新尝试并安全取代旧尝试](adr/022-user-initiated-join-supersession.md)
- [ADR-023：旧资料独立化与重新配对](adr/023-legacy-profile-isolation-and-re-pairing.md)
