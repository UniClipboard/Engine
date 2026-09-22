use super::*;

/// 一条配对记录尚欠的全部收尾工作。
///
/// “配对是否仍打开”“是否阻止新准入”“下一步恢复动作”“截止时间”只从这里派生；
/// 调用方不再自行组合记录状态。新增状态或终态义务时，只需更新本文件中的穷尽匹配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionOutstandingWork {
    obligations: Vec<AdmissionObligation>,
    next_step: Option<AdmissionRecoveryStep>,
    deadline_ms: Option<i64>,
    missing_deadline: bool,
}

/// 配对记录尚欠的单项工作，按执行先后排列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionObligation {
    /// 协议尚未进入终态。
    ProtocolInFlight,
    /// 加入方已完成本机激活，仍需取得最终确认。
    SettlementPending,
    /// 本机终止后仍需隔离目标空间。
    LocalSpaceIsolation,
    /// 本机终止后仍需投递放弃通知。
    AbandonmentNotice,
    /// 邀请方正式提交后仍在等待对端确认。
    SponsorConfirmation,
    /// 邀请方拒绝或到期后仍需核对并撤销该次加入。
    SponsorRevocation,
    /// 无法安全继续，需要恢复处理。
    RecoveryRequired,
}

/// 恢复流程对一条记录应执行的下一步。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionRecoveryStep {
    SponsorRevocation,
    SponsorConfirmation,
    SponsorDeadline,
    JoinerNetwork,
    JoinerExpiry,
    CompletionHelperDeadline,
}

impl AdmissionObligation {
    /// 该工作存在时，空间仍处于配对中；后台收尾不算配对仍在进行。
    pub const fn holds_pairing_open(self) -> bool {
        match self {
            Self::ProtocolInFlight
            | Self::SettlementPending
            | Self::LocalSpaceIsolation
            | Self::SponsorConfirmation => true,
            Self::AbandonmentNotice | Self::SponsorRevocation | Self::RecoveryRequired => false,
        }
    }

    /// 该工作存在时，不允许开启另一条准入事实。
    pub const fn blocks_new_admission(self) -> bool {
        match self {
            Self::ProtocolInFlight
            | Self::SettlementPending
            | Self::AbandonmentNotice
            | Self::SponsorRevocation
            | Self::RecoveryRequired => true,
            Self::LocalSpaceIsolation | Self::SponsorConfirmation => false,
        }
    }
}

impl AdmissionOutstandingWork {
    pub fn obligations(&self) -> &[AdmissionObligation] {
        &self.obligations
    }

    pub fn is_settled(&self) -> bool {
        self.obligations.is_empty()
    }

    pub fn holds_pairing_open(&self) -> bool {
        self.obligations
            .iter()
            .any(|obligation| obligation.holds_pairing_open())
    }

    pub fn blocks_new_admission(&self) -> bool {
        self.obligations
            .iter()
            .any(|obligation| obligation.blocks_new_admission())
    }

    pub const fn next_step(&self) -> Option<AdmissionRecoveryStep> {
        self.next_step
    }

    /// 下一步恢复动作的截止时间；撤销与无动作记录没有截止时间。
    pub const fn deadline_ms(&self) -> Option<i64> {
        self.deadline_ms
    }

    /// 未终结记录缺少共同期限，无法自动收尾，需要处理。
    pub const fn missing_deadline(&self) -> bool {
        self.missing_deadline
    }
}

impl SpaceAdmissionAggregate {
    pub fn outstanding_work(&self) -> AdmissionOutstandingWork {
        let next_step = self.next_recovery_step();
        let deadline_ms = match next_step {
            Some(
                AdmissionRecoveryStep::JoinerNetwork
                | AdmissionRecoveryStep::JoinerExpiry
                | AdmissionRecoveryStep::SponsorConfirmation
                | AdmissionRecoveryStep::SponsorDeadline
                | AdmissionRecoveryStep::CompletionHelperDeadline,
            ) => self.expires_at_ms(),
            Some(AdmissionRecoveryStep::SponsorRevocation) | None => None,
        };
        let missing_deadline = !self.is_terminal()
            && (self.expires_at_ms().is_none()
                || (self.record_role() == Some(AdmissionRole::Sponsor)
                    && self.sponsor_pairing_confirmation().is_none()
                    && !self.has_expirable_sponsor()));
        AdmissionOutstandingWork {
            obligations: self.obligations(),
            next_step,
            deadline_ms,
            missing_deadline,
        }
    }

    fn obligations(&self) -> Vec<AdmissionObligation> {
        let mut obligations = Vec::new();
        match &self.state {
            SpaceAdmissionRecordState::Joiner(_)
            | SpaceAdmissionRecordState::Sponsor(_)
            | SpaceAdmissionRecordState::CompletionHelper(_) => {
                obligations.push(AdmissionObligation::ProtocolInFlight);
            }
            SpaceAdmissionRecordState::Terminal(terminal) => match terminal {
                SpaceAdmissionTerminalState::Active(
                    SpaceAdmissionActiveState::PendingSettlement(_),
                ) => obligations.push(AdmissionObligation::SettlementPending),
                SpaceAdmissionTerminalState::Active(SpaceAdmissionActiveState::Settled(_))
                | SpaceAdmissionTerminalState::Superseded(_) => {}
                SpaceAdmissionTerminalState::Terminated(state) => {
                    if let Some(cleanup) = &state.cleanup {
                        if cleanup.local_space_transition.is_some() {
                            obligations.push(AdmissionObligation::LocalSpaceIsolation);
                        }
                        if cleanup.pending_exchange.is_some() {
                            obligations.push(AdmissionObligation::AbandonmentNotice);
                        }
                    }
                }
                SpaceAdmissionTerminalState::Completed(state) => {
                    if state.confirmation.is_some_and(|summary| {
                        summary.status == SponsorPairingConfirmationStatus::AwaitingPeerConfirmation
                    }) {
                        obligations.push(AdmissionObligation::SponsorConfirmation);
                    }
                }
                SpaceAdmissionTerminalState::Rejected(SpaceAdmissionRejectedState::Sponsor(
                    state,
                )) => {
                    if requires_revocation(state.abandonment_cleanup.as_ref()) {
                        obligations.push(AdmissionObligation::SponsorRevocation);
                    }
                }
                SpaceAdmissionTerminalState::Rejected(
                    SpaceAdmissionRejectedState::LocalJoiner(_)
                    | SpaceAdmissionRejectedState::Joiner(_),
                ) => {}
                SpaceAdmissionTerminalState::SponsorExpired(state) => {
                    if requires_revocation(Some(&state.abandonment_cleanup)) {
                        obligations.push(AdmissionObligation::SponsorRevocation);
                    }
                }
                SpaceAdmissionTerminalState::RecoveryRequired(_) => {
                    obligations.push(AdmissionObligation::RecoveryRequired);
                }
            },
        }
        obligations
    }

    /// 恢复动作按固定优先级选择；持久恢复索引缓存该结论，调整规则须同步升级索引格式。
    fn next_recovery_step(&self) -> Option<AdmissionRecoveryStep> {
        let sponsor_confirmation_pending =
            self.sponsor_pairing_confirmation().is_some_and(|summary| {
                summary.status == SponsorPairingConfirmationStatus::AwaitingPeerConfirmation
            });
        if self.has_pending_sponsor_abandonment() {
            Some(AdmissionRecoveryStep::SponsorRevocation)
        } else if sponsor_confirmation_pending {
            Some(AdmissionRecoveryStep::SponsorConfirmation)
        } else if self.has_expirable_sponsor() {
            Some(AdmissionRecoveryStep::SponsorDeadline)
        } else if self.pending_recovery().is_some()
            || self.invitation_resolution().is_some()
            || self.has_pending_local_termination()
        {
            Some(AdmissionRecoveryStep::JoinerNetwork)
        } else if self.has_expirable_local_join() {
            Some(AdmissionRecoveryStep::JoinerExpiry)
        } else if self.record_role() == Some(AdmissionRole::CompletionHelper)
            && self.expires_at_ms().is_some()
        {
            Some(AdmissionRecoveryStep::CompletionHelperDeadline)
        } else {
            None
        }
    }
}

const fn requires_revocation(cleanup: Option<&SponsorAbandonmentCleanup>) -> bool {
    matches!(
        cleanup,
        Some(SponsorAbandonmentCleanup::Known(_) | SponsorAbandonmentCleanup::Unknown { .. })
    )
}
