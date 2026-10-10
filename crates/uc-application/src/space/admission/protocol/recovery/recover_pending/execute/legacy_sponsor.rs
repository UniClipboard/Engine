use super::super::model::AdmissionRecoveryReport;
use super::super::{AdmissionRecoveryCommitToken, AdmissionRecoveryTrigger};
use crate::space::admission::protocol::AdmissionRecoveryService;
use uc_core::membership::{LegacySponsorMembership, SponsorAdmission, SponsorRecordStage};
use uc_observability_contract::{uc_info, uc_warn};

impl AdmissionRecoveryService {
    /// 收尾一条早于尝试期限格式写入的邀请方记录。
    ///
    /// 这类记录没有期限，计时永远不会结束它，却会阻止所有新的准入。结论只来自记录阶段和成员账本：
    /// 不会写账本的阶段直接关闭；其他阶段必须取得账本证据：没有候选事件则无成员，已有则关闭为未确认，账本不可读时保持原记录等待下一轮。
    pub(super) async fn close_legacy_sponsor(
        &self,
        aggregate: SponsorAdmission,
        token: AdmissionRecoveryCommitToken,
        trigger: AdmissionRecoveryTrigger,
        report: &mut AdmissionRecoveryReport,
    ) {
        let trigger = trigger_label(trigger);
        let stage = stage_label(aggregate.stage());
        let membership = match aggregate.legacy_member_query() {
            Ok(None) => LegacySponsorMembership::Absent,
            Ok(Some(query)) => match self.membership.load().await {
                Ok(view) => {
                    let present = view.space().is_some_and(|space| {
                        space
                            .history()
                            .contains_event_id(query.add_event_id().as_bytes())
                    });
                    if present {
                        LegacySponsorMembership::Present
                    } else {
                        LegacySponsorMembership::Absent
                    }
                }
                Err(_) => {
                    // 账本暂不可读时不能判断成员是否已提交：保持原记录，下一轮重新评估。
                    report.deferred_count += 1;
                    uc_warn!(
                        trigger = trigger,
                        state = stage,
                        reason = "legacy_no_deadline",
                        outcome = "deferred_member_evidence",
                        "legacy Sponsor admission record kept because the member ledger is unavailable"
                    );
                    return;
                }
            },
            Err(_) => {
                report.recovery_required_count += 1;
                uc_warn!(
                    trigger = trigger,
                    state = stage,
                    reason = "legacy_no_deadline",
                    outcome = "invalid_record",
                    "legacy Sponsor admission record cannot be classified"
                );
                return;
            }
        };
        let outcome = match membership {
            LegacySponsorMembership::Absent => "closed_without_member",
            LegacySponsorMembership::Present => "closed_unconfirmed_member",
        };
        match aggregate.close_legacy(membership) {
            Ok(Some(transition)) => {
                match self
                    .commit_sponsor_deadline_and_notify(token, transition)
                    .await
                {
                    Ok(_) => {
                        report.advanced_count += 1;
                        uc_info!(
                            trigger = trigger,
                            state = stage,
                            reason = "legacy_no_deadline",
                            outcome = outcome,
                            "legacy Sponsor admission record closed"
                        );
                    }
                    Err(error) => {
                        self.record_state_error(report, error);
                        uc_warn!(
                            trigger = trigger,
                            state = stage,
                            reason = "legacy_no_deadline",
                            outcome = "commit_not_saved",
                            "legacy Sponsor admission record could not be saved"
                        );
                    }
                }
            }
            Ok(None) => {}
            Err(_) => {
                report.recovery_required_count += 1;
                uc_warn!(
                    trigger = trigger,
                    state = stage,
                    reason = "legacy_no_deadline",
                    outcome = "invalid_record",
                    "legacy Sponsor admission record cannot be closed"
                );
            }
        }
    }
}

const fn trigger_label(trigger: AdmissionRecoveryTrigger) -> &'static str {
    match trigger {
        AdmissionRecoveryTrigger::Startup => "startup",
        AdmissionRecoveryTrigger::Resume => "resume",
        AdmissionRecoveryTrigger::Periodic => "periodic",
        AdmissionRecoveryTrigger::StateChanged => "state_changed",
    }
}

const fn stage_label(stage: Option<SponsorRecordStage>) -> &'static str {
    match stage {
        Some(SponsorRecordStage::Accepted) => "accepted",
        Some(SponsorRecordStage::Candidate) => "candidate",
        Some(SponsorRecordStage::Committed) => "committed",
        Some(SponsorRecordStage::Applied) => "applied",
        None => "terminal",
    }
}
