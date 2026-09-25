//! 成员效果的单步执行：按效果当前阶段调用对应能力，完成后由 Owner 推进阶段。

use std::sync::Arc;

use async_trait::async_trait;
use uc_core::membership::{MemberEffectKind, MemberEffectPhase, UnfinishedMemberEffect};

use crate::space::membership::{
    ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    MembershipEffectExecutionError, ResolveRePairingPort,
};

/// 加入效果激活成功后才清除重新配对提示。
pub(crate) struct RePairingAwareMembershipActivation {
    inner: Arc<dyn ActivateMembershipEffectPort>,
    re_pairing: Arc<dyn ResolveRePairingPort>,
}

impl RePairingAwareMembershipActivation {
    pub(crate) fn new(
        inner: Arc<dyn ActivateMembershipEffectPort>,
        re_pairing: Arc<dyn ResolveRePairingPort>,
    ) -> Self {
        Self { inner, re_pairing }
    }
}

#[async_trait]
impl ActivateMembershipEffectPort for RePairingAwareMembershipActivation {
    async fn activate_membership_effect(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        self.inner.activate_membership_effect(effect).await?;
        if effect.kind() == MemberEffectKind::AddDevice {
            self.re_pairing
                .resolve_after_successful_pairing()
                .await
                .map_err(MembershipEffectExecutionError::deferred_from)?;
        }
        Ok(())
    }
}

pub(super) struct MembershipEffectSteps {
    member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
    security: Arc<dyn ApplyMembershipSecurityPort>,
    activation: Arc<dyn ActivateMembershipEffectPort>,
}

impl MembershipEffectSteps {
    pub(super) fn new(
        member_facts: Arc<dyn ApplyMembershipMemberFactsPort>,
        security: Arc<dyn ApplyMembershipSecurityPort>,
        activation: Arc<dyn ActivateMembershipEffectPort>,
    ) -> Self {
        Self {
            member_facts,
            security,
            activation,
        }
    }

    /// 执行效果当前阶段的一步。各能力按事件幂等，重复执行不会产生第二份结果。
    pub(super) async fn run(
        &self,
        effect: &UnfinishedMemberEffect,
    ) -> Result<(), MembershipEffectExecutionError> {
        tracing::debug!(
            kind = ?effect.kind(),
            phase = ?effect.phase(),
            affected_device_count = effect.affected_device_ids().len(),
            "开始执行成员效果阶段"
        );
        match effect.phase() {
            MemberEffectPhase::Prepared => self.member_facts.apply_member_facts(effect).await,
            MemberEffectPhase::MemberFactsApplied => {
                self.security.apply_membership_security(effect).await
            }
            MemberEffectPhase::SecurityApplied => {
                self.activation.activate_membership_effect(effect).await
            }
        }
    }
}
