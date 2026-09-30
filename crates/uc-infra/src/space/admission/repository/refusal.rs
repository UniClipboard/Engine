use uc_core::membership::AdmissionObligation;
use uc_observability_contract::log_safe_errors;

use super::SpaceAdmissionStateStoreError;

/// 准入状态仓储拒绝一次变更的固定原因；文本只含固定词与枚举名，可直接进入本地日志。
#[derive(Debug, thiserror::Error)]
pub(crate) enum AdmissionRefusal {
    #[error("space admission refused: unsettled_attempt {record} {obligation}")]
    UnsettledAttempt {
        record: &'static str,
        obligation: &'static str,
    },
}

impl AdmissionRefusal {
    pub(crate) fn unsettled(sponsor_record: bool, obligation: AdmissionObligation) -> Self {
        Self::UnsettledAttempt {
            record: if sponsor_record {
                "sponsor_record"
            } else {
                "joiner_record"
            },
            obligation: obligation_label(obligation),
        }
    }
}

fn obligation_label(obligation: AdmissionObligation) -> &'static str {
    match obligation {
        AdmissionObligation::ProtocolInFlight => "protocol_in_flight",
        AdmissionObligation::SettlementPending => "settlement_pending",
        AdmissionObligation::LocalSpaceIsolation => "local_space_isolation",
        AdmissionObligation::AbandonmentNotice => "abandonment_notice",
        AdmissionObligation::SponsorConfirmation => "sponsor_confirmation",
        AdmissionObligation::SponsorRevocation => "sponsor_revocation",
        AdmissionObligation::RecoveryRequired => "recovery_required",
    }
}

log_safe_errors!(pub(crate) fn repo_error_layers => [
    SpaceAdmissionStateStoreError,
    AdmissionRefusal,
]);
