use std::slice;
use std::sync::Arc;
use std::time::Duration;

use sha2::{Digest, Sha256};
use uc_core::ids::{DeviceId, SpaceId};
use uc_core::membership::{
    AdmissionMemberBindingV2, LedgerInput, LedgerMemberStatus, MemberInstanceId, MembershipEventId,
    MembershipHistoryV2ReceiveOutcome, VersionedMembershipHistory,
};

use crate::space::membership::{
    ledger_error, CurrentMemberSignatureError, CurrentMemberSignaturePort, DeviceTrustStatus,
    MembershipLedgerError, MembershipMaintenanceStepOutcome, MembershipOwner, MembershipView,
    QueryDeviceTrustUseCase, RecoverMembershipEffectsPort,
};

use super::{
    AdmissionAbandonmentRevocationTarget, AdmissionRevocationPort, AdmissionRevocationResult,
    AdmissionRevocationTarget, MembershipCommitReceipt, RemoveSpaceMemberError,
    RemoveSpaceMemberResult,
};

const COMMITTED_STATUS_QUERY_ATTEMPTS: usize = 3;
const COMMITTED_STATUS_QUERY_BACKOFF: Duration = Duration::from_millis(100);

pub(crate) struct RemoveSpaceMemberUseCase {
    owner: Arc<MembershipOwner>,
    signer: Arc<dyn CurrentMemberSignaturePort>,
    query: Arc<QueryDeviceTrustUseCase>,
    effects: Arc<dyn RecoverMembershipEffectsPort>,
    execution_lock: tokio::sync::Mutex<()>,
}

#[derive(Clone)]
struct ExactRemovalTarget {
    space_id: SpaceId,
    member_instance_id: MemberInstanceId,
    origin: RemovalTargetOrigin,
    operation_id: [u8; 16],
}

#[derive(Clone, Copy)]
enum RemovalTargetOrigin {
    ActivationBaseline,
    Admission(MembershipEventId),
}

#[derive(Clone, Copy)]
enum ExactRemovalKind {
    Removed,
    AlreadyAbsent,
}

struct ExactRemovalCommit {
    kind: ExactRemovalKind,
    change_id: MembershipEventId,
    receipt: MembershipCommitReceipt,
}

impl RemoveSpaceMemberUseCase {
    pub(crate) fn new(
        owner: Arc<MembershipOwner>,
        signer: Arc<dyn CurrentMemberSignaturePort>,
        query: Arc<QueryDeviceTrustUseCase>,
        effects: Arc<dyn RecoverMembershipEffectsPort>,
    ) -> Self {
        Self {
            owner,
            signer,
            query,
            effects,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        target_device_id: &DeviceId,
    ) -> Result<RemoveSpaceMemberResult, RemoveSpaceMemberError> {
        let _guard = self.execution_lock.lock().await;
        let target = self.resolve_device_target(target_device_id).await?;
        let committed = self.execute_exact_with_retry(target).await?;
        // 移除已提交；本地效果能完成多少就先完成多少，其余由执行器按持久待办继续。
        let _ = self.effects.recover_membership_effects().await;
        self.public_result(committed).await
    }

    async fn resolve_device_target(
        &self,
        target_device_id: &DeviceId,
    ) -> Result<ExactRemovalTarget, RemoveSpaceMemberError> {
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let space = view.require_space().map_err(map_ledger_error)?;
        let history = space.history();
        if target_device_id == space.local_device_id() {
            return Err(RemoveSpaceMemberError::SelfTarget);
        }
        // 重复请求可能落在上一次已提交的移除之后；已被本历史移除的设备按已不存在处理，
        // 让调用方得到同一结果，而不是把已完成的移除误报为目标不存在。
        let member_instance_id = history
            .member_for_device(target_device_id, slice::from_ref(target_device_id))
            .filter(|member| {
                history.effective_members().contains(member)
                    || history.removal_event_id_for(*member).is_some()
            })
            .ok_or(RemoveSpaceMemberError::TargetNotFound)?;
        let origin = match history.admission_event_id_for(member_instance_id) {
            Some(event_id) => RemovalTargetOrigin::Admission(event_id),
            None => RemovalTargetOrigin::ActivationBaseline,
        };
        Ok(ExactRemovalTarget {
            space_id: SpaceId::from_str(history.lineage_id()),
            member_instance_id,
            origin,
            operation_id: uuid::Uuid::new_v4().into_bytes(),
        })
    }

    async fn execute_exact_with_retry(
        &self,
        target: ExactRemovalTarget,
    ) -> Result<ExactRemovalCommit, RemoveSpaceMemberError> {
        match self.execute_exact_once(target.clone()).await {
            Err(RemoveSpaceMemberError::StateChanged) => self.execute_exact_once(target).await,
            result => result,
        }
    }

    async fn execute_exact_once(
        &self,
        target: ExactRemovalTarget,
    ) -> Result<ExactRemovalCommit, RemoveSpaceMemberError> {
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let space = view.require_space().map_err(map_ledger_error)?;
        let history = space.history();
        let local_device_id = *space.local_device_id();
        let local_member = space.local_member();
        if space.ledger().local_status() != LedgerMemberStatus::Active {
            return Err(RemoveSpaceMemberError::LocalMemberRemoved);
        }
        if local_member == target.member_instance_id {
            return Err(RemoveSpaceMemberError::SelfTarget);
        }
        if !target.matches(history) {
            return Err(RemoveSpaceMemberError::TargetNotFound);
        }
        if !history
            .effective_members()
            .contains(&target.member_instance_id)
        {
            let change_id = history
                .removal_event_id_for(target.member_instance_id)
                .ok_or(RemoveSpaceMemberError::TargetNotFound)?;
            return Ok(ExactRemovalCommit {
                kind: ExactRemovalKind::AlreadyAbsent,
                change_id,
                receipt: receipt(&view)?,
            });
        }
        let credential = self
            .signer
            .current_membership_credential(&local_device_id)
            .await
            .map_err(map_signature_error)?;
        if credential.member_instance_id(&local_device_id) != local_member {
            return Err(RemoveSpaceMemberError::RecoveryRequired);
        }
        let history_digest = history
            .current_position()
            .map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?
            .history_digest;
        let mut event = history
            .create_unsigned_local_removal_event(
                local_member,
                &credential,
                target.member_instance_id,
                target.operation_id,
                history_digest,
            )
            .map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?;
        event.signature = self
            .signer
            .sign_current_member_payload(&event.signing_payload())
            .await
            .map_err(map_signature_error)?;
        let change_id = event.event_id();
        let verifier = self.owner.verifier_handle();
        let committed = self
            .owner
            .commit(move |draft| {
                let space = draft.require_space()?;
                let mut history = space.history().clone();
                // 签名期间历史已前进：按状态变化重新判定，而不是把新历史当作损坏。
                if !target.matches(&history) || history.current_head() != event.parent_event_id {
                    return Err(MembershipLedgerError::Conflict);
                }
                if history
                    .verify_and_receive_event(event, verifier.as_ref())
                    .map_err(|_| MembershipLedgerError::Corrupt)?
                    != MembershipHistoryV2ReceiveOutcome::Applied
                {
                    return Err(MembershipLedgerError::Corrupt);
                }
                let retained_device_ids = history
                    .effective_members()
                    .into_iter()
                    .filter(|member| {
                        *member != target.member_instance_id && *member != local_member
                    })
                    .filter_map(|member| history.admission_facts_for(member))
                    .map(|facts| facts.device_id)
                    .collect::<Vec<_>>();
                draft
                    .apply(LedgerInput::LocalRemovalSigned {
                        history,
                        retained_device_ids,
                    })
                    .map_err(ledger_error)
            })
            .await
            .map_err(map_ledger_error)?;
        Ok(ExactRemovalCommit {
            kind: ExactRemovalKind::Removed,
            change_id,
            receipt: receipt(&committed.view)?,
        })
    }

    async fn public_result(
        &self,
        committed: ExactRemovalCommit,
    ) -> Result<RemoveSpaceMemberResult, RemoveSpaceMemberError> {
        let change_id = committed.change_id;
        let status = self
            .query_committed_status()
            .await
            .ok_or(RemoveSpaceMemberError::CommittedButPending { change_id })?;
        Ok(RemoveSpaceMemberResult {
            change_id,
            commit: committed.receipt,
            status,
        })
    }

    /// 移除已经提交，状态查询只负责回显结果；短暂的读取竞争不能把已完成的移除报告为失败。
    async fn query_committed_status(&self) -> Option<DeviceTrustStatus> {
        for attempt in 0..COMMITTED_STATUS_QUERY_ATTEMPTS {
            if attempt > 0 {
                tokio::time::sleep(COMMITTED_STATUS_QUERY_BACKOFF).await;
            }
            if let Ok(status) = self.query.execute().await {
                return Some(status);
            }
        }
        None
    }

    async fn execute_admission_revocation(
        &self,
        target: AdmissionRevocationTarget,
    ) -> Result<AdmissionRevocationResult, RemoveSpaceMemberError> {
        let binding = target.member_binding();
        let exact = ExactRemovalTarget {
            space_id: binding.space_id().clone(),
            member_instance_id: binding.member_instance_id(),
            origin: RemovalTargetOrigin::Admission(binding.add_event_id()),
            operation_id: admission_revocation_operation_id(&target),
        };
        let committed = self.execute_exact_with_retry(exact).await?;
        let effects = self.effects.recover_membership_effects().await;
        if effects != MembershipMaintenanceStepOutcome::Completed {
            return Ok(AdmissionRevocationResult::LocalEffectsPending {
                change_id: committed.change_id,
            });
        }
        Ok(match committed.kind {
            ExactRemovalKind::Removed => AdmissionRevocationResult::Removed {
                change_id: committed.change_id,
            },
            ExactRemovalKind::AlreadyAbsent => AdmissionRevocationResult::AlreadyAbsent {
                change_id: committed.change_id,
            },
        })
    }
}

#[async_trait::async_trait]
impl AdmissionRevocationPort for RemoveSpaceMemberUseCase {
    async fn revoke_admission(
        &self,
        target: AdmissionRevocationTarget,
    ) -> Result<AdmissionRevocationResult, RemoveSpaceMemberError> {
        let _guard = self.execution_lock.lock().await;
        self.execute_admission_revocation(target).await
    }

    async fn revoke_abandoned_admission(
        &self,
        target: AdmissionAbandonmentRevocationTarget,
    ) -> Result<AdmissionRevocationResult, RemoveSpaceMemberError> {
        let _guard = self.execution_lock.lock().await;
        let view = self.owner.load().await.map_err(map_ledger_error)?;
        let history = view.require_space().map_err(map_ledger_error)?.history();
        let binding = AdmissionMemberBindingV2::new(
            target.attempt_digest(),
            SpaceId::from_str(history.lineage_id()),
            target.member_instance_id(),
            target.add_event_id(),
        )
        .map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?;
        self.execute_admission_revocation(AdmissionRevocationTarget::new(
            target.admission_id(),
            binding,
        ))
        .await
    }
}

impl ExactRemovalTarget {
    fn matches(&self, history: &VersionedMembershipHistory) -> bool {
        if history.lineage_id() != self.space_id.as_ref()
            || history
                .admission_facts_for(self.member_instance_id)
                .is_none()
        {
            return false;
        }
        match self.origin {
            RemovalTargetOrigin::ActivationBaseline => history
                .admission_event_id_for(self.member_instance_id)
                .is_none(),
            RemovalTargetOrigin::Admission(add_event_id) => {
                history.admission_event_id_for(self.member_instance_id) == Some(add_event_id)
            }
        }
    }
}

/// 提交回执只来自已提交状态；摘要无法计算说明记录已损坏。
fn receipt(view: &MembershipView) -> Result<MembershipCommitReceipt, RemoveSpaceMemberError> {
    let space = view
        .require_space()
        .map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?;
    Ok(MembershipCommitReceipt {
        revision: view.revision(),
        history_digest: space
            .history_digest()
            .map_err(|_| RemoveSpaceMemberError::RecoveryRequired)?,
    })
}

fn admission_revocation_operation_id(target: &AdmissionRevocationTarget) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(b"uniclipboard/admission-revocation-operation/v1\0");
    hasher.update(target.admission_id().as_bytes());
    hasher.update(target.member_binding().digest());
    let digest = hasher.finalize();
    let mut operation_id = [0; 16];
    operation_id.copy_from_slice(&digest[..16]);
    operation_id
}

fn map_ledger_error(error: MembershipLedgerError) -> RemoveSpaceMemberError {
    match error {
        MembershipLedgerError::Locked => RemoveSpaceMemberError::Locked,
        MembershipLedgerError::Conflict => RemoveSpaceMemberError::StateChanged,
        MembershipLedgerError::Corrupt | MembershipLedgerError::RecoveryRequired => {
            RemoveSpaceMemberError::RecoveryRequired
        }
        MembershipLedgerError::Unavailable => RemoveSpaceMemberError::Unavailable,
    }
}

fn map_signature_error(error: CurrentMemberSignatureError) -> RemoveSpaceMemberError {
    match error {
        CurrentMemberSignatureError::InvalidState => RemoveSpaceMemberError::RecoveryRequired,
        CurrentMemberSignatureError::Unavailable | CurrentMemberSignatureError::Repository(_) => {
            RemoveSpaceMemberError::Unavailable
        }
    }
}
