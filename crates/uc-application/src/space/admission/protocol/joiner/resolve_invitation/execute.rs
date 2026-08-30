use uc_core::membership::{
    AdmissionRetryState, PendingAdmissionExchange, SpaceAdmissionMessageKind,
};
use uc_core::membership::{JoinerAdmission, JoinerInvitationResolution};

use crate::space::admission::protocol::{
    AdmissionRecoveryCommitToken, AdmissionRecoveryReport, AdmissionRecoveryService,
    JoinerAdmissionService, JoinerStartMaterialError,
};

impl JoinerAdmissionService {
    pub(in crate::space::admission::protocol) async fn recover_invitation_resolution(
        &self,
        recovery: &AdmissionRecoveryService,
        report: &mut AdmissionRecoveryReport,
        aggregate: JoinerAdmission,
        token: AdmissionRecoveryCommitToken,
    ) {
        let state = match aggregate.invitation_resolution() {
            Some(JoinerInvitationResolution::Ready { .. }) => ResolutionState::Ready,
            Some(JoinerInvitationResolution::Started { .. }) => ResolutionState::Started,
            Some(JoinerInvitationResolution::Resolved { .. }) => ResolutionState::Resolved,
            None => {
                report.recovery_required_count += 1;
                return;
            }
        };

        match state {
            ResolutionState::Ready => {
                let started = match aggregate.mark_invitation_resolution_started() {
                    Ok(started) => started,
                    Err(_) => {
                        report.recovery_required_count += 1;
                        return;
                    }
                };
                let (transition, short_code) = started.into_parts();
                let committed = match recovery.commit_recovery(token, transition).await {
                    Ok(committed) => committed,
                    Err(error) => {
                        recovery.record_state_error(report, error);
                        return;
                    }
                };
                report.advanced_count += 1;

                let resolved = self.resolve_invitation.resolve_once(&short_code).await;
                let resolution_succeeded = resolved.is_ok();
                let (aggregate, token) = committed.into_parts();
                let transition = match resolved {
                    Ok(full_invitation) => aggregate.save_resolved_invitation(full_invitation),
                    Err(_) => aggregate.reject_started_invitation_resolution(),
                };
                let transition = match transition {
                    Ok(transition) => transition,
                    Err(_) => {
                        report.recovery_required_count += 1;
                        return;
                    }
                };
                match recovery.commit_recovery(token, transition).await {
                    Ok(_) if resolution_succeeded => {
                        report.advanced_count += 1;
                        report.deferred_count += 1;
                        self.maintenance_wake.wake();
                    }
                    Ok(_) => report.rejected_count += 1,
                    Err(error) => recovery.record_state_error(report, error),
                }
            }
            ResolutionState::Started => {
                let transition = match aggregate.reject_started_invitation_resolution() {
                    Ok(transition) => transition,
                    Err(_) => {
                        report.recovery_required_count += 1;
                        return;
                    }
                };
                match recovery.commit_recovery(token, transition).await {
                    Ok(_) => report.rejected_count += 1,
                    Err(error) => recovery.record_state_error(report, error),
                }
            }
            ResolutionState::Resolved => {
                let Some(JoinerInvitationResolution::Resolved {
                    full_invitation,
                    start_context,
                }) = aggregate.invitation_resolution()
                else {
                    report.recovery_required_count += 1;
                    return;
                };
                let material = self
                    .start_material
                    .create_resolved(
                        aggregate.admission_id(),
                        aggregate.join_id(),
                        full_invitation,
                        start_context,
                    )
                    .await;
                let material = match material {
                    Ok(material) => material,
                    Err(JoinerStartMaterialError::Unavailable { .. }) => {
                        report.deferred_count += 1;
                        return;
                    }
                    Err(JoinerStartMaterialError::InvalidInvitation) => {
                        report.recovery_required_count += 1;
                        return;
                    }
                };
                let (
                    admission_id,
                    join_id,
                    route,
                    join_request,
                    private_state,
                    encrypted_password_equivalent,
                ) = material.into_parts();
                if admission_id != aggregate.admission_id() || join_id != aggregate.join_id() {
                    report.recovery_required_count += 1;
                    return;
                }
                let pending_exchange = match PendingAdmissionExchange::new(
                    route,
                    join_request,
                    SpaceAdmissionMessageKind::Candidate,
                    match AdmissionRetryState::new(0, 0) {
                        Ok(retry) => retry,
                        Err(_) => {
                            report.recovery_required_count += 1;
                            return;
                        }
                    },
                ) {
                    Ok(pending) => pending,
                    Err(_) => {
                        report.recovery_required_count += 1;
                        return;
                    }
                };
                let transition = match aggregate.start_resolved_join(
                    private_state,
                    encrypted_password_equivalent,
                    pending_exchange,
                ) {
                    Ok(transition) => transition,
                    Err(_) => {
                        report.recovery_required_count += 1;
                        return;
                    }
                };
                match recovery.commit_recovery(token, transition).await {
                    Ok(_) => {
                        report.advanced_count += 1;
                        self.maintenance_wake.wake();
                    }
                    Err(error) => recovery.record_state_error(report, error),
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum ResolutionState {
    Ready,
    Started,
    Resolved,
}
