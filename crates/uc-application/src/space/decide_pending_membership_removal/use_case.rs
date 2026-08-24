use uc_core::membership::{
    CurrentMemberSignatureError, MemberInstanceId, MembershipCredential,
    MembershipDecisionStoreOutcome, MembershipDecisionV2, MembershipEventId, MembershipEventV2,
    MembershipHistoryRelationship, MembershipOperationV2, RemovalDecision, SpaceMembershipState,
    VersionedMembershipHistory, WorkspaceConvergenceEvent,
};

use super::{
    DecidePendingMembershipRemovalDeps, DecidePendingMembershipRemovalError,
    DecidePendingMembershipRemovalResult,
};
use crate::space::membership_history::LoadedMembershipHistory;

enum LoadedMembershipRemoval {
    AlreadyDecided(CommittedMembershipRemovalDecision),

    NoLongerPending {
        current_removal_event_id: Option<MembershipEventId>,
    },

    Pending(PendingMembershipRemoval),
}

/// 已验证且仍等待本机决定的成员移除。
struct PendingMembershipRemoval {
    /// 已验证、携带加载版本且尚未写回的当前成员历史。
    loaded_history: LoadedMembershipHistory,

    /// 用户正在决定的成员移除事件。
    removal_event: MembershipEventV2,

    /// 当前本机成员的签名凭据。
    local_credential: MembershipCredential,

    /// 当前本机成员在历史中的稳定身份。
    local_member_instance: MemberInstanceId,
}

/// 已验证、等待按加载版本提交的本机移除决定。
struct PreparedRemovalDecisionCommit {
    loaded_history: LoadedMembershipHistory,
    removal_event_id: MembershipEventId,
    removal_author_member_instance: MemberInstanceId,
    decision: RemovalDecision,
}

struct CommittedMembershipRemovalDecision {
    history: VersionedMembershipHistory,
    removal_event_id: MembershipEventId,
    removal_author_member_instance: MemberInstanceId,
    decision: RemovalDecision,
}

enum RemovalDecisionCommitOutcome {
    Committed(CommittedMembershipRemovalDecision),
    HistoryChanged,
}

impl PendingMembershipRemoval {
    fn requires_self_removal_confirmation(
        &self,
        decision: RemovalDecision,
        confirm_self_removal: bool,
    ) -> bool {
        decision == RemovalDecision::Accept
            && !confirm_self_removal
            && matches!(
                &self.removal_event.operation,
                MembershipOperationV2::RemoveDevice
                { member }
                    if *member == self.local_member_instance
            )
    }
}

/// 决定一项从其他成员收到、已经验证但尚未应用的成员移除。
///
/// 同一个活动 Space 的决定必须串行执行，避免接受和拒绝同时基于
/// 同一份成员历史完成签名。跨进程冲突仍由历史保存时的版本比较处理。
pub(crate) struct DecidePendingMembershipRemovalUseCase {
    deps: DecidePendingMembershipRemovalDeps,
    execution_lock: tokio::sync::Mutex<()>,
}

impl DecidePendingMembershipRemovalUseCase {
    pub(crate) fn new(deps: DecidePendingMembershipRemovalDeps) -> Self {
        Self {
            deps,
            execution_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) async fn execute(
        &self,
        removal_event_id: MembershipEventId,
        decision: RemovalDecision,
        confirm_self_removal: bool,
    ) -> Result<DecidePendingMembershipRemovalResult, DecidePendingMembershipRemovalError> {
        let _guard = self.execution_lock.lock().await;

        match self.load_pending_removal(removal_event_id).await? {
            LoadedMembershipRemoval::AlreadyDecided(committed) => {
                self.apply_committed_removal_effects(&committed).await?;
                let status = self.query_membership_status().await?;
                Ok(DecidePendingMembershipRemovalResult::AlreadyDecided {
                    removal_event_id,
                    decision: committed.decision,
                    status,
                })
            }
            LoadedMembershipRemoval::NoLongerPending {
                current_removal_event_id,
            } => {
                let status = self.query_membership_status().await?;
                Ok(
                    DecidePendingMembershipRemovalResult::PendingRemovalChanged {
                        current_removal_event_id,
                        status,
                    },
                )
            }
            LoadedMembershipRemoval::Pending(pending) => {
                if pending.requires_self_removal_confirmation(decision, confirm_self_removal) {
                    let status = self.query_membership_status().await?;
                    return Ok(
                        DecidePendingMembershipRemovalResult::SelfRemovalConfirmationRequired {
                            removal_event_id,
                            status,
                        },
                    );
                }

                let signed_decision = self.sign_removal_decision(&pending, decision).await?;
                let prepared = self.prepare_history_commit(pending, signed_decision)?;
                let committed = match self.commit_removal_decision(prepared).await? {
                    RemovalDecisionCommitOutcome::HistoryChanged => {
                        let status = self.query_membership_status().await?;
                        return Ok(
                            DecidePendingMembershipRemovalResult::PendingRemovalChanged {
                                current_removal_event_id: status
                                    .current_change
                                    .as_ref()
                                    .map(|change| change.change_id),
                                status,
                            },
                        );
                    }
                    RemovalDecisionCommitOutcome::Committed(committed) => committed,
                };

                self.apply_committed_removal_effects(&committed).await?;
                let status = self.query_membership_status().await?;
                Ok(match decision {
                    RemovalDecision::Accept => DecidePendingMembershipRemovalResult::Accepted {
                        removal_event_id,
                        status,
                    },
                    RemovalDecision::Reject => DecidePendingMembershipRemovalResult::Rejected {
                        removal_event_id,
                        status,
                    },
                })
            }
        }
    }

    async fn load_pending_removal(
        &self,
        removal_event_id: MembershipEventId,
    ) -> Result<LoadedMembershipRemoval, DecidePendingMembershipRemovalError> {
        let loaded_history = self
            .deps
            .membership_history
            .load_verified_history()
            .await
            .map_err(map_history_repository_error)?
            .ok_or(DecidePendingMembershipRemovalError::Unavailable)?;
        let history = loaded_history.history();

        let local_credential = self
            .deps
            .member_signatures
            .current_membership_credential(&self.deps.own_device)
            .await
            .map_err(map_member_signature_error)?;

        let local_member_instance = local_credential.member_instance_id(&self.deps.own_device);

        if let Some(completed) = history.decision_for(removal_event_id, local_member_instance) {
            let decision = completed.decision;
            let removal_author_member_instance = history
                .event(removal_event_id)
                .map(|event| event.author_member_instance_id)
                .ok_or(DecidePendingMembershipRemovalError::Corrupt)?;
            return Ok(LoadedMembershipRemoval::AlreadyDecided(
                CommittedMembershipRemovalDecision {
                    removal_event_id,
                    removal_author_member_instance,
                    decision,
                    history: loaded_history.into_history(),
                },
            ));
        }

        let current_removal_event_id = history.pending_removal_decision(local_member_instance);

        if current_removal_event_id != Some(removal_event_id) {
            return Ok(LoadedMembershipRemoval::NoLongerPending {
                current_removal_event_id,
            });
        }

        let removal_event = history
            .event(removal_event_id)
            .cloned()
            .ok_or(DecidePendingMembershipRemovalError::Corrupt)?;

        if !matches!(
            &removal_event.operation,
            MembershipOperationV2::RemoveDevice { .. }
        ) {
            return Err(DecidePendingMembershipRemovalError::Corrupt);
        }

        Ok(LoadedMembershipRemoval::Pending(PendingMembershipRemoval {
            loaded_history,
            removal_event,
            local_credential,
            local_member_instance,
        }))
    }

    async fn sign_removal_decision(
        &self,
        pending: &PendingMembershipRemoval,
        decision: RemovalDecision,
    ) -> Result<MembershipDecisionV2, DecidePendingMembershipRemovalError> {
        let mut signed_decision = pending
            .loaded_history
            .history()
            .create_unsigned_local_removal_decision(
                pending.removal_event.event_id(),
                pending.local_member_instance,
                &pending.local_credential,
                decision,
                uuid::Uuid::new_v4().into_bytes(),
            )
            .map_err(|_| DecidePendingMembershipRemovalError::Corrupt)?;

        signed_decision.signature = self
            .deps
            .member_signatures
            .sign_current_member_payload(&signed_decision.signing_payload())
            .await
            .map_err(map_member_signature_error)?;

        Ok(signed_decision)
    }

    fn prepare_history_commit(
        &self,
        mut pending: PendingMembershipRemoval,
        signed_decision: MembershipDecisionV2,
    ) -> Result<PreparedRemovalDecisionCommit, DecidePendingMembershipRemovalError> {
        let removal_event_id = pending.removal_event.event_id();
        let removal_author_member_instance = pending.removal_event.author_member_instance_id;
        let decision = signed_decision.decision;

        let outcome = pending
            .loaded_history
            .apply_signed_local_removal_decision(signed_decision, pending.local_member_instance)
            .map_err(map_history_repository_error)?;
        if outcome != MembershipDecisionStoreOutcome::Stored {
            return Err(DecidePendingMembershipRemovalError::Corrupt);
        }

        Ok(PreparedRemovalDecisionCommit {
            loaded_history: pending.loaded_history,
            removal_event_id,
            removal_author_member_instance,
            decision,
        })
    }

    async fn commit_removal_decision(
        &self,
        prepared: PreparedRemovalDecisionCommit,
    ) -> Result<RemovalDecisionCommitOutcome, DecidePendingMembershipRemovalError> {
        match self
            .deps
            .membership_history
            .commit(prepared.loaded_history)
            .await
        {
            Ok(committed_history) => {
                self.deps.recovery_requests.request();
                Ok(RemovalDecisionCommitOutcome::Committed(
                    CommittedMembershipRemovalDecision {
                        history: committed_history.into_history(),
                        removal_event_id: prepared.removal_event_id,
                        removal_author_member_instance: prepared.removal_author_member_instance,
                        decision: prepared.decision,
                    },
                ))
            }
            Err(crate::space::membership_history::MembershipHistoryRepositoryError::Conflict) => {
                Ok(RemovalDecisionCommitOutcome::HistoryChanged)
            }
            Err(error) => Err(map_history_repository_error(error)),
        }
    }

    async fn apply_committed_removal_effects(
        &self,
        committed: &CommittedMembershipRemovalDecision,
    ) -> Result<(), DecidePendingMembershipRemovalError> {
        let _state_guard = self.deps.state_write_lock.lock().await;
        let mut state = self
            .deps
            .state_repository
            .load_state()
            .await
            .map_err(map_state_repository_error)?
            .ok_or(DecidePendingMembershipRemovalError::Unavailable)?;
        if state.space_lineage != committed.history.lineage_id() {
            return Err(DecidePendingMembershipRemovalError::Corrupt);
        }

        let removal_author = committed
            .history
            .admission_facts_for(committed.removal_author_member_instance)
            .map(|facts| facts.device_id.clone())
            .ok_or(DecidePendingMembershipRemovalError::Corrupt)?;
        let relationship = match committed.decision {
            RemovalDecision::Accept => MembershipHistoryRelationship::Consistent,
            RemovalDecision::Reject => MembershipHistoryRelationship::Diverged,
        };
        let (_, effect) = state
            .apply(
                WorkspaceConvergenceEvent::PeerHistoryRelationshipUpdated {
                    peer: removal_author,
                    relationship,
                },
                self.deps.clock.now_ms(),
            )
            .map_err(|_| DecidePendingMembershipRemovalError::Corrupt)?;

        if effect.persist {
            self.deps
                .state_repository
                .save_state(&state)
                .await
                .map_err(map_state_repository_error)?;
        }
        if effect.publish {
            self.deps.state_events.publish(&state);
        }
        Ok(())
    }

    async fn query_membership_status(
        &self,
    ) -> Result<
        crate::space::query_space_membership_status::SpaceMembershipStatus,
        DecidePendingMembershipRemovalError,
    > {
        self.deps
            .membership_status_query
            .execute()
            .await
            .map_err(|error| match error {
                crate::space::query_space_membership_status::QuerySpaceMembershipStatusError::Unavailable => {
                    DecidePendingMembershipRemovalError::Unavailable
                }
                crate::space::query_space_membership_status::QuerySpaceMembershipStatusError::Corrupt => {
                    DecidePendingMembershipRemovalError::Corrupt
                }
                crate::space::query_space_membership_status::QuerySpaceMembershipStatusError::Failed => {
                    DecidePendingMembershipRemovalError::Failed
                }
            })
    }
}

fn map_history_repository_error(
    error: crate::space::membership_history::MembershipHistoryRepositoryError,
) -> DecidePendingMembershipRemovalError {
    match error {
        crate::space::membership_history::MembershipHistoryRepositoryError::Locked => {
            DecidePendingMembershipRemovalError::Unavailable
        }
        crate::space::membership_history::MembershipHistoryRepositoryError::Corrupt => {
            DecidePendingMembershipRemovalError::Corrupt
        }
        crate::space::membership_history::MembershipHistoryRepositoryError::Conflict
        | crate::space::membership_history::MembershipHistoryRepositoryError::Unavailable => {
            DecidePendingMembershipRemovalError::Failed
        }
    }
}

fn map_member_signature_error(
    error: CurrentMemberSignatureError,
) -> DecidePendingMembershipRemovalError {
    match error {
        CurrentMemberSignatureError::Unavailable => {
            DecidePendingMembershipRemovalError::Unavailable
        }
        CurrentMemberSignatureError::InvalidState => DecidePendingMembershipRemovalError::Corrupt,
        CurrentMemberSignatureError::Repository(_) => DecidePendingMembershipRemovalError::Failed,
    }
}

fn map_state_repository_error(
    error: crate::space::membership_state::SpaceMembershipStateRepositoryError,
) -> DecidePendingMembershipRemovalError {
    match error {
        crate::space::membership_state::SpaceMembershipStateRepositoryError::Locked => {
            DecidePendingMembershipRemovalError::Unavailable
        }
        crate::space::membership_state::SpaceMembershipStateRepositoryError::Corrupt => {
            DecidePendingMembershipRemovalError::Corrupt
        }
        crate::space::membership_state::SpaceMembershipStateRepositoryError::Unavailable => {
            DecidePendingMembershipRemovalError::Failed
        }
    }
}
