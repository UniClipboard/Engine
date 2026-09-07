//! 验证成员证据并完成本机关系处理，回复与已保存结果来自同一动作。

mod use_case;

pub(crate) use use_case::ReconcileMembershipEvidenceUseCase;

use uc_core::membership::{MembershipConflictEvidenceV3, MembershipHistoryRelationship};

pub(crate) struct MembershipEvidenceExchange {
    pub(crate) response: MembershipConflictEvidenceV3,
    pub(crate) relationship: MembershipHistoryRelationship,
}
