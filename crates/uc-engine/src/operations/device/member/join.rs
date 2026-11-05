//! 加入状态对外投影。

use super::*;

pub(crate) fn join_space_status(status: CurrentJoinStatus) -> JoinSpaceStatusSummary {
    match status {
        CurrentJoinStatus::Active {
            join_id,
            joined_space,
            peer_upgrade_required,
        } => JoinSpaceStatusSummary::Active {
            join_id: encode_join_id(join_id),
            joined_space: JoinedSpaceSummary {
                sponsor_device_id: joined_space.sponsor_device_id.to_string(),
                sponsor_identity_fingerprint: joined_space
                    .sponsor_identity_fingerprint
                    .as_display()
                    .to_string(),
                space_id: joined_space.space_id,
                self_device_id: joined_space.self_device_id.to_string(),
                self_identity_fingerprint: joined_space
                    .self_identity_fingerprint
                    .as_display()
                    .to_string(),
                migrated_records: joined_space.migrated_records,
                preserved_unreadable_records: joined_space.preserved_unreadable_records,
            },
            peer_upgrade_required,
        },
        CurrentJoinStatus::Pending {
            join_id,
            target_space_id,
            sponsor_device_id,
            sponsor_identity_fingerprint,
            cancel_requested,
            peer_upgrade_required,
        } => JoinSpaceStatusSummary::Pending {
            join_id: encode_join_id(join_id),
            target_space_id,
            sponsor_device_id: sponsor_device_id.map(|device_id| device_id.to_string()),
            sponsor_identity_fingerprint: sponsor_identity_fingerprint
                .map(|fingerprint| fingerprint.as_display().to_string()),
            cancel_requested,
            peer_upgrade_required,
        },
        CurrentJoinStatus::Processing {
            join_id,
            target_space_id,
            sponsor_device_id,
            sponsor_identity_fingerprint,
            peer_upgrade_required,
        } => JoinSpaceStatusSummary::Processing {
            join_id: encode_join_id(join_id),
            target_space_id,
            sponsor_device_id: sponsor_device_id.to_string(),
            sponsor_identity_fingerprint: sponsor_identity_fingerprint.as_display().to_string(),
            peer_upgrade_required,
        },
        CurrentJoinStatus::NeedsAttention {
            join_id,
            reason,
            recovery,
            next_retry_at_ms,
        } => JoinSpaceStatusSummary::NeedsAttention {
            join_id: encode_join_id(join_id),
            reason: match reason {
                uc_application::facade::JoinSpaceAttentionReason::OutcomeCannotBeProven => {
                    JoinSpaceAttentionReasonSummary::OutcomeCannotBeProven
                }
                uc_application::facade::JoinSpaceAttentionReason::ContinuationUnavailable => {
                    JoinSpaceAttentionReasonSummary::ContinuationUnavailable
                }
            },
            recovery: match recovery {
                uc_application::facade::JoinSpaceAttentionRecovery::PreserveDataAndContactSupport => {
                    JoinSpaceAttentionRecoverySummary::PreserveDataAndContactSupport
                }
                uc_application::facade::JoinSpaceAttentionRecovery::RestartWithNewInvitation => {
                    JoinSpaceAttentionRecoverySummary::RestartWithNewInvitation
                }
            },
            next_retry_at_ms,
        },
        CurrentJoinStatus::Rejected { join_id, reason } => JoinSpaceStatusSummary::Rejected {
            join_id: encode_join_id(join_id),
            reason: match reason {
                uc_core::membership::SpaceAdmissionRejectionReason::InvitationUnavailable => {
                    JoinSpaceRejectionReasonSummary::InvitationUnavailable
                }
                uc_core::membership::SpaceAdmissionRejectionReason::AuthenticationRejected => {
                    JoinSpaceRejectionReasonSummary::AuthenticationRejected
                }
                uc_core::membership::SpaceAdmissionRejectionReason::SponsorUnresponsive => {
                    JoinSpaceRejectionReasonSummary::SponsorUnresponsive
                }
                uc_core::membership::SpaceAdmissionRejectionReason::IdentityConflict => {
                    JoinSpaceRejectionReasonSummary::IdentityConflict
                }
                uc_core::membership::SpaceAdmissionRejectionReason::BaseHistoryChanged => {
                    JoinSpaceRejectionReasonSummary::BaseHistoryChanged
                }
                uc_core::membership::SpaceAdmissionRejectionReason::JoinerHistoryAhead => {
                    JoinSpaceRejectionReasonSummary::JoinerHistoryAhead
                }
                uc_core::membership::SpaceAdmissionRejectionReason::HistoryConflict => {
                    JoinSpaceRejectionReasonSummary::HistoryConflict
                }
                uc_core::membership::SpaceAdmissionRejectionReason::CompletionInvalid => {
                    JoinSpaceRejectionReasonSummary::CompletionInvalid
                }
                uc_core::membership::SpaceAdmissionRejectionReason::MembershipHistoryInvalid => {
                    JoinSpaceRejectionReasonSummary::MembershipHistoryInvalid
                }
                uc_core::membership::SpaceAdmissionRejectionReason::SecurityMaterialInvalid => {
                    JoinSpaceRejectionReasonSummary::SecurityMaterialInvalid
                }
                uc_core::membership::SpaceAdmissionRejectionReason::RelationshipConflict => {
                    JoinSpaceRejectionReasonSummary::RelationshipConflict
                }
                uc_core::membership::SpaceAdmissionRejectionReason::ActivationStateInvalid => {
                    JoinSpaceRejectionReasonSummary::ActivationStateInvalid
                }
                uc_core::membership::SpaceAdmissionRejectionReason::PeerUpgradeRequired => {
                    JoinSpaceRejectionReasonSummary::PeerUpgradeRequired
                }
                uc_core::membership::SpaceAdmissionRejectionReason::Cancelled => {
                    JoinSpaceRejectionReasonSummary::Cancelled
                }
                uc_core::membership::SpaceAdmissionRejectionReason::RemovedBeforeActivation => {
                    JoinSpaceRejectionReasonSummary::RemovedBeforeActivation
                }
            },
        },
        CurrentJoinStatus::Terminated { join_id, reason } => JoinSpaceStatusSummary::Terminated {
            join_id: encode_join_id(join_id),
            reason: match reason {
                JoinSpaceTerminationReason::Cancelled => {
                    JoinSpaceTerminationReasonSummary::Cancelled
                }
                JoinSpaceTerminationReason::Expired => JoinSpaceTerminationReasonSummary::Expired,
                JoinSpaceTerminationReason::Superseded => {
                    JoinSpaceTerminationReasonSummary::Superseded
                }
            },
        },
    }
}

fn encode_join_id(join_id: [u8; 16]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(join_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_status_preserves_the_peer_upgrade_prompt() {
        let summary = join_space_status(CurrentJoinStatus::Pending {
            join_id: [0x31; 16],
            target_space_id: None,
            sponsor_device_id: None,
            sponsor_identity_fingerprint: None,
            cancel_requested: false,
            peer_upgrade_required: true,
        });

        assert!(matches!(
            summary,
            JoinSpaceStatusSummary::Pending {
                peer_upgrade_required: true,
                ..
            }
        ));
    }

    #[test]
    fn final_confirmation_wait_is_reported_as_processing() {
        let summary = join_space_status(CurrentJoinStatus::Processing {
            join_id: [0x32; 16],
            target_space_id: "space-a".to_owned(),
            sponsor_device_id: uc_core::DeviceId::new("sponsor"),
            sponsor_identity_fingerprint:
                uc_core::security::IdentityFingerprint::from_display_string("ABCD-EFGH-IJKL-MNOP")
                    .expect("fingerprint"),
            peer_upgrade_required: false,
        });

        assert!(matches!(
            summary,
            JoinSpaceStatusSummary::Processing {
                ref target_space_id,
                ref sponsor_device_id,
                ..
            } if target_space_id == "space-a" && sponsor_device_id == "sponsor"
        ));
    }

    #[test]
    fn join_needing_attention_has_a_stable_public_status() {
        let summary = join_space_status(CurrentJoinStatus::NeedsAttention {
            join_id: [0x41; 16],
            reason: uc_application::facade::JoinSpaceAttentionReason::OutcomeCannotBeProven,
            recovery:
                uc_application::facade::JoinSpaceAttentionRecovery::PreserveDataAndContactSupport,
            next_retry_at_ms: None,
        });

        assert!(matches!(
            summary,
            JoinSpaceStatusSummary::NeedsAttention {
                join_id,
                reason: JoinSpaceAttentionReasonSummary::OutcomeCannotBeProven,
                recovery: JoinSpaceAttentionRecoverySummary::PreserveDataAndContactSupport,
                next_retry_at_ms: None,
            } if join_id == encode_join_id([0x41; 16])
        ));
    }

    #[test]
    fn terminated_join_status_is_not_reported_as_pending_or_rejected() {
        let summary = join_space_status(CurrentJoinStatus::Terminated {
            join_id: [0x41; 16],
            reason: JoinSpaceTerminationReason::Expired,
        });

        assert_eq!(
            summary,
            JoinSpaceStatusSummary::Terminated {
                join_id: "QUFBQUFBQUFBQUFBQUFBQQ".to_owned(),
                reason: JoinSpaceTerminationReasonSummary::Expired,
            }
        );
    }
}
