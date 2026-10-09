use uc_core::error_class::ErrorClass;
use uc_core::membership::AdmissionObligation;

/// 准入状态仓储拒绝一次变更的固定原因；文本与分类只含固定词与枚举名，可直接进入本地日志。
#[derive(Debug, thiserror::Error)]
pub(crate) enum AdmissionRefusal {
    #[error("space admission refused: unsettled_attempt {record} {obligation}", record = record_label(*sponsor_record), obligation = obligation_label(*obligation))]
    UnsettledAttempt {
        sponsor_record: bool,
        obligation: AdmissionObligation,
    },
}

impl AdmissionRefusal {
    pub(crate) fn unsettled(sponsor_record: bool, obligation: AdmissionObligation) -> Self {
        Self::UnsettledAttempt {
            sponsor_record,
            obligation,
        }
    }
}

impl ErrorClass for AdmissionRefusal {
    fn class(&self) -> &'static str {
        match self {
            Self::UnsettledAttempt {
                sponsor_record: true,
                obligation,
            } => match obligation {
                AdmissionObligation::ProtocolInFlight => {
                    "unsettled_sponsor_record_protocol_in_flight"
                }
                AdmissionObligation::SettlementPending => {
                    "unsettled_sponsor_record_settlement_pending"
                }
                AdmissionObligation::LocalSpaceIsolation => {
                    "unsettled_sponsor_record_local_space_isolation"
                }
                AdmissionObligation::AbandonmentNotice => {
                    "unsettled_sponsor_record_abandonment_notice"
                }
                AdmissionObligation::SponsorConfirmation => {
                    "unsettled_sponsor_record_sponsor_confirmation"
                }
                AdmissionObligation::SponsorRevocation => {
                    "unsettled_sponsor_record_sponsor_revocation"
                }
                AdmissionObligation::RecoveryRequired => {
                    "unsettled_sponsor_record_recovery_required"
                }
            },
            Self::UnsettledAttempt {
                sponsor_record: false,
                obligation,
            } => match obligation {
                AdmissionObligation::ProtocolInFlight => {
                    "unsettled_joiner_record_protocol_in_flight"
                }
                AdmissionObligation::SettlementPending => {
                    "unsettled_joiner_record_settlement_pending"
                }
                AdmissionObligation::LocalSpaceIsolation => {
                    "unsettled_joiner_record_local_space_isolation"
                }
                AdmissionObligation::AbandonmentNotice => {
                    "unsettled_joiner_record_abandonment_notice"
                }
                AdmissionObligation::SponsorConfirmation => {
                    "unsettled_joiner_record_sponsor_confirmation"
                }
                AdmissionObligation::SponsorRevocation => {
                    "unsettled_joiner_record_sponsor_revocation"
                }
                AdmissionObligation::RecoveryRequired => {
                    "unsettled_joiner_record_recovery_required"
                }
            },
        }
    }
}

fn record_label(sponsor_record: bool) -> &'static str {
    if sponsor_record {
        "sponsor_record"
    } else {
        "joiner_record"
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn every_record_and_obligation_pair_has_its_own_class_matching_the_display_text() {
        let obligations = [
            AdmissionObligation::ProtocolInFlight,
            AdmissionObligation::SettlementPending,
            AdmissionObligation::LocalSpaceIsolation,
            AdmissionObligation::AbandonmentNotice,
            AdmissionObligation::SponsorConfirmation,
            AdmissionObligation::SponsorRevocation,
            AdmissionObligation::RecoveryRequired,
        ];
        let mut seen = HashSet::new();
        for sponsor_record in [true, false] {
            for obligation in obligations {
                let refusal = AdmissionRefusal::unsettled(sponsor_record, obligation);
                let text = refusal.to_string();
                let suffix = text
                    .strip_prefix("space admission refused: ")
                    .expect("fixed prefix")
                    .replace(' ', "_");
                assert_eq!(
                    refusal.class(),
                    suffix.replace("unsettled_attempt_", "unsettled_")
                );
                assert!(seen.insert(refusal.class()));
            }
        }
        assert_eq!(seen.len(), 14);
    }
}
