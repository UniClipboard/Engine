use std::time::Duration;

use super::*;
use crate::space::membership::{
    DeviceTrustStatus, QueryDeviceTrustError, QueryMembershipConflictStatusPort,
    ResolveMembershipConflictInput, ResolveMembershipConflictResult,
    ResolveMembershipConflictUseCase,
};
use crate::test_support::membership_scenario::{finish, require, scenario};

#[derive(Clone, Copy)]
enum Failure {
    Rejected,
    Invalid,
    Unavailable,
    Storage,
    Generation,
    SubmitRejected,
}

struct FailingChannel {
    failure: Failure,
    records: Arc<MemoryMembershipRecords>,
    calls: AtomicUsize,
}

#[async_trait]
impl MembershipBranchRecoveryChannelPort for FailingChannel {
    fn freeze(
        &self,
        _peer: DeviceId,
    ) -> Result<Arc<dyn BranchRecoveryOperationScopePort>, MembershipBranchRecoveryChannelError>
    {
        Ok(Arc::new(TestRecoveryScope(Arc::new(AtomicBool::new(
            false,
        )))))
    }

    async fn request_membership_branch_group_info(
        &self,
        _request: MembershipBranchRecoveryRequest,
    ) -> Result<Vec<u8>, MembershipBranchRecoveryChannelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.failure {
            Failure::Storage => self.records.fail_next_commits(1),
            Failure::Generation => self.records.replace_database(self.records.record()),
            Failure::SubmitRejected => panic!("prepared recipient must not refetch group info"),
            _ => {}
        }
        let source = anyhow::anyhow!("injected recovery failure");
        Err(match self.failure {
            Failure::Invalid => MembershipBranchRecoveryChannelError::Invalid { source },
            Failure::Unavailable => MembershipBranchRecoveryChannelError::Unavailable { source },
            _ => MembershipBranchRecoveryChannelError::Rejected { source },
        })
    }

    async fn submit_membership_branch_external_commit(
        &self,
        _request: MembershipBranchRecoveryCommit,
    ) -> Result<MembershipBranchRecoveryPackageV1, MembershipBranchRecoveryChannelError> {
        assert!(matches!(self.failure, Failure::SubmitRejected));
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(MembershipBranchRecoveryChannelError::Rejected {
            source: anyhow::anyhow!("injected external commit rejection"),
        })
    }
}

struct UnusedStatusQuery;

#[async_trait]
impl QueryMembershipConflictStatusPort for UnusedStatusQuery {
    async fn query_status(&self) -> Result<DeviceTrustStatus, QueryDeviceTrustError> {
        panic!("a failed selection must not be reported as completed")
    }
}

fn recovery(
    fixture: &Fixture,
    owner: Arc<MembershipOwner>,
    channel: Arc<FailingChannel>,
) -> RecoverMembershipConflictUseCase {
    RecoverMembershipConflictUseCase::new(
        owner,
        channel,
        fixture.recipient.clone(),
        fixture.transition.clone(),
        fixture.transition.clone(),
        Arc::new(AcceptingVerifier),
        Arc::new(FixedClock),
    )
}

#[tokio::test]
async fn rejected_selection_is_terminal_across_query_restart_and_reselection() {
    let scenario = scenario(
        "rejected-selection-terminal",
        0x0040_1106,
        Duration::from_secs(5),
        "just cargo test -p uc-application rejected_selection_is_terminal -- --nocapture",
    );
    let result = async {
        for failure in [Failure::Rejected, Failure::Invalid, Failure::SubmitRejected] {
            let fixture = fixture();
            if matches!(failure, Failure::SubmitRejected) {
                fixture.repository.edit(|space| {
                    let conflict = &space.branch_recovery.conflicts[&fixture.conflict_id];
                    space.branch_recovery.recovery_sessions.insert(
                        fixture.transition_id,
                        MembershipBranchRecoverySession::new_recipient_prepared(
                            fixture.transition_id,
                            fixture.conflict_id,
                            conflict.selected_branch_id.unwrap(),
                            space.ledger.local_member,
                            vec![0x61],
                            vec![0x62],
                        )
                        .unwrap(),
                    );
                });
            }
            let before = fixture.repository.space();
            let channel = Arc::new(FailingChannel {
                failure,
                records: fixture.repository.records.clone(),
                calls: AtomicUsize::new(0),
            });
            let recovering = recovery(&fixture, fixture.repository.owner.clone(), channel.clone());
            require(
                recovering.execute().await == RecoverMembershipConflictOutcome::StableFailure,
                "stable-failure-not-classified",
            )?;
            let persisted = fixture.repository.space();
            require(
                persisted.branch_recovery.conflicts[&fixture.conflict_id].status
                    == MembershipConflictStatus::RePairingRequired,
                "stable-failure-left-selection-pending",
            )?;
            require(
                persisted.ledger.history == before.ledger.history
                    && persisted.branch_recovery.recovery_sessions
                        == before.branch_recovery.recovery_sessions
                    && persisted.branch_recovery.branch_transitions.is_empty(),
                "stable-failure-changed-membership",
            )?;
            scenario.record_event("terminal-result-persisted-without-membership-change");

            let restarted = OwnerFixture::new(fixture.repository.records.record());
            let resolver = ResolveMembershipConflictUseCase::new(
                restarted.owner.clone(),
                Arc::new(UnusedStatusQuery),
            );
            let query = resolver.query().await.unwrap();
            require(
                query.conflicts.len() == 1
                    && query.conflicts[0].status == MembershipConflictStatus::RePairingRequired
                    && !query.conflicts[0].local_resolution_completed,
                "terminal-query-hid-or-completed-conflict",
            )?;
            let target = persisted.branch_recovery.conflicts[&fixture.conflict_id]
                .selected_branch_id
                .unwrap();
            require(
                matches!(
                    resolver
                        .execute(ResolveMembershipConflictInput {
                            conflict_id: fixture.conflict_id,
                            target_branch_id: target,
                        })
                        .await
                        .unwrap(),
                    ResolveMembershipConflictResult::RePairingRequired { .. }
                ),
                "reselection-returned-pending",
            )?;
            let restarted_recovery = recovery(&fixture, restarted.owner, channel.clone());
            for _ in 0..3 {
                require(
                    restarted_recovery.execute().await
                        == RecoverMembershipConflictOutcome::Completed,
                    "terminal-conflict-still-scheduled",
                )?;
            }
            require(
                channel.calls.load(Ordering::SeqCst) == 1,
                "terminal-recovery-repeated-network-attempt",
            )?;
            scenario
                .record_event("restart-query-reselection-and-maintenance-retain-terminal-result");
        }
        Ok(())
    }
    .await;
    finish(scenario, result);
}

struct InvalidAdvance;

#[async_trait]
impl AdvanceMembershipBranchTransitionPort for InvalidAdvance {
    async fn advance_membership_branch_transition(
        &self,
        _input: AdvanceMembershipBranchTransitionInput,
    ) -> Result<MembershipBranchTransitionV1, AdvanceMembershipBranchTransitionError> {
        Err(AdvanceMembershipBranchTransitionError::Invalid {
            source: anyhow::anyhow!("injected committed transition failure"),
        })
    }
}

#[tokio::test]
async fn committed_transition_is_not_abandoned_after_stable_failure() {
    let scenario = scenario(
        "committed-selection-retains-transition",
        0x0040_1108,
        Duration::from_secs(5),
        "just cargo test -p uc-application committed_transition_is_not_abandoned -- --nocapture",
    );
    let result = async {
        let fixture = fixture();
        require(
            fixture.use_case.execute().await == RecoverMembershipConflictOutcome::Completed,
            "transition-fixture-did-not-commit",
        )?;
        let before = fixture.repository.records.record();
        let recovery = RecoverMembershipConflictUseCase::new(
            fixture.repository.owner.clone(),
            fixture.recovery.clone(),
            fixture.recipient.clone(),
            fixture.transition.clone(),
            Arc::new(InvalidAdvance),
            Arc::new(AcceptingVerifier),
            Arc::new(FixedClock),
        );
        require(
            recovery.execute().await == RecoverMembershipConflictOutcome::StableFailure,
            "committed-failure-not-classified",
        )?;
        require(
            fixture.repository.records.record() == before,
            "committed-transition-was-abandoned",
        )?;
        scenario.record_event("committed-transition-retained-after-stable-failure");
        require(
            fixture.use_case.execute().await == RecoverMembershipConflictOutcome::Completed,
            "committed-transition-could-not-resume",
        )?;
        require(
            fixture.repository.space().branch_recovery.conflicts[&fixture.conflict_id].status
                == MembershipConflictStatus::Completed,
            "committed-transition-did-not-finish",
        )?;
        scenario.record_event("committed-transition-resumed-to-completion");
        Ok(())
    }
    .await;
    finish(scenario, result);
}

#[tokio::test]
async fn unavailable_storage_and_generation_changes_do_not_commit_terminal_failure() {
    let scenario = scenario(
        "rejected-selection-failure-boundaries",
        0x0040_1107,
        Duration::from_secs(5),
        "just cargo test -p uc-application unavailable_storage_and_generation -- --nocapture",
    );
    let result = async {
        for failure in [Failure::Unavailable, Failure::Storage, Failure::Generation] {
            let fixture = fixture();
            let before = fixture.repository.records.record();
            let channel = Arc::new(FailingChannel {
                failure,
                records: fixture.repository.records.clone(),
                calls: AtomicUsize::new(0),
            });
            let recovering = recovery(&fixture, fixture.repository.owner.clone(), channel);
            require(
                recovering.execute().await == RecoverMembershipConflictOutcome::Deferred,
                "temporary-or-retired-work-was-terminal",
            )?;
            require(
                fixture.repository.records.record() == before,
                "temporary-or-retired-work-overwrote-state",
            )?;
            scenario.record_event("temporary-failure-preserved-pending-selection");
        }
        Ok(())
    }
    .await;
    finish(scenario, result);
}
