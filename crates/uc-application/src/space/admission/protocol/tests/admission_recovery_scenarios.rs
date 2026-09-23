use std::time::Duration;

use uc_core::membership::{SpaceAdmissionMessageKind, SponsorPairingConfirmationStatus};

use super::super::sponsor::HandleAuthenticatedSpaceAdmissionMessagePort;
use super::super::test_support::{
    authenticated_applied, authenticated_complete_ack, authenticated_join_request,
    authenticated_prepared, ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use super::super::AdmissionRecoveryTrigger;
use crate::space::membership::{
    MembershipMaintenanceStepOutcome, MembershipMaintenanceTrigger, RecoverSpaceAdmissionsPort,
};
use crate::space::JoinSpaceInput;
use crate::test_support::membership_scenario::{finish, require, scenario};

use super::support::PairingScenarioFixture;

const RETRY_REPRODUCE: &str = "cargo nextest run -p uc-application -E 'test(final_confirmation_retry_yields_ordinary_maintenance)'";
const PAIRING_FIXTURE_REPRODUCE: &str =
    "cargo nextest run -p uc-application -E 'test(joiner_pairing_fixture_reaches_active_settled)'";
const VISIBILITY_REPRODUCE: &str = "cargo nextest run -p uc-application -E 'test(three_device_confirmation_has_one_visible_admission)'";
const RESTART_REPRODUCE: &str =
    "cargo nextest run -p uc-application -E 'test(restart_continues_from_persisted_admission)'";

#[tokio::test]
async fn joiner_pairing_fixture_reaches_active_settled() {
    let scenario = scenario(
        "joiner-pairing-fixture",
        0x0040_3402,
        Duration::from_secs(1),
        PAIRING_FIXTURE_REPRODUCE,
    );
    let result = async {
        let pairing = PairingScenarioFixture::prepare().await;
        let snapshot = {
            let _stage = scenario.stage("complete-joiner-pairing");
            pairing
                .complete_joiner_pairing(join_input("pairing-fixture"))
                .await
                .map_err(|failure| fixture(failure.condition()))?
        };
        scenario.record_event("joiner-pairing-completed");
        require(snapshot.is_active(), "joiner-pairing-was-not-active")?;
        require(
            snapshot.final_confirmation_complete(),
            "joiner-pairing-final-confirmation-incomplete",
        )
    }
    .await;
    finish(scenario, result);
}

#[tokio::test]
async fn final_confirmation_retry_yields_ordinary_maintenance() {
    let scenario = scenario(
        "final-confirmation-retry",
        0x0040_0001,
        Duration::from_secs(1),
        RETRY_REPRODUCE,
    );
    let result = async {
        let pair = SpaceAdmissionProtocolTestPair::upgrade_once_on_settlement().await;
        pair.joiner()
            .start_join_at(join_input("final-confirmation-retry"), 1_000)
            .await
            .map_err(|_| fixture("join-start"))?;
        let activation_plan = pair
            .joiner()
            .recover_space_admissions(&MembershipMaintenanceTrigger::StateChanged)
            .await;
        scenario.record_event("activation-plan-saved");
        require(
            activation_plan.step() == MembershipMaintenanceStepOutcome::Completed
                && !activation_plan.should_continue(),
            "ordinary-maintenance-ran-before-activation",
        )?;
        {
            let _stage = scenario.stage("complete-activation");
            pair.joiner()
                .complete_pending_space_transition()
                .await
                .map_err(|_| fixture("activation"))?;
        }

        let blocked = pair
            .joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
        scenario.record_event("complete-ack-retry-pending");
        require(
            blocked.peer_upgrade_required_count == 1 && blocked.recovery_required_count == 0,
            "final-confirmation-failure-was-not-retryable",
        )?;
        require(
            pair.saved_join().peer_upgrade_required(),
            "retryable-final-confirmation-was-not-persisted",
        )?;

        let recovered = pair
            .joiner()
            .recover_pending(AdmissionRecoveryTrigger::PeerOnline(
                uc_core::DeviceId::new("upgraded-peer"),
            ))
            .await;
        scenario.record_event("final-confirmation-settled");
        require(
            recovered.peer_upgrade_required_count == 0 && recovered.recovery_required_count == 0,
            "final-confirmation-retry-did-not-recover",
        )?;
        require(
            pair.saved_join().is_active_settled(),
            "final-confirmation-did-not-settle",
        )
    }
    .await;
    finish(scenario, result);
}

#[tokio::test]
async fn three_device_confirmation_has_one_visible_admission() {
    let scenario = scenario(
        "three-device-confirmation-visibility",
        0x0040_0003,
        Duration::from_secs(2),
        VISIBILITY_REPRODUCE,
    );
    let result = async {
        let pair = SpaceAdmissionProtocolTestPair::fresh().await;
        let candidate = pair
            .sponsor()
            .handle(authenticated_join_request())
            .await
            .map_err(|_| fixture("candidate"))?;
        let prepared = authenticated_prepared(
            candidate
                .envelope()
                .ok_or_else(|| fixture("candidate-envelope"))?,
        );
        pair.seed_sponsor(candidate.into_admission());
        let commit = pair
            .sponsor()
            .handle(prepared)
            .await
            .map_err(|_| fixture("commit"))?;
        let applied = authenticated_applied(
            commit
                .envelope()
                .ok_or_else(|| fixture("commit-envelope"))?,
        );
        pair.seed_sponsor(commit.into_admission());
        let complete = pair
            .sponsor()
            .handle(applied)
            .await
            .map_err(|_| fixture("complete"))?;
        let complete_ack = authenticated_complete_ack(
            complete
                .envelope()
                .ok_or_else(|| fixture("complete-envelope"))?,
        );
        pair.seed_sponsor(complete.into_admission());

        pair.set_now_ms(301_000);
        pair.recover_sponsor().await;
        scenario.record_event("third-device-unconfirmed");
        require(
            pair.sponsor_confirmation_status()
                == Some(SponsorPairingConfirmationStatus::Unconfirmed),
            "third-device-became-confirmed-before-complete-ack",
        )?;

        let settled = pair
            .sponsor()
            .handle(complete_ack)
            .await
            .map_err(|_| fixture("settled"))?;
        pair.seed_sponsor(settled.into_admission());
        scenario.record_event("third-device-confirmed-once");
        require(
            pair.sponsor_confirmation_status() == Some(SponsorPairingConfirmationStatus::Confirmed),
            "third-device-not-confirmed-after-complete-ack",
        )?;
        require(
            pair.events()
                .iter()
                .filter(|event| **event == ProtocolEvent::SponsorSavedCompleted)
                .count()
                == 1,
            "third-device-admission-was-not-unique",
        )
    }
    .await;
    finish(scenario, result);
}

#[tokio::test]
async fn restart_continues_from_persisted_admission() {
    let scenario = scenario(
        "restart-from-persistent-state",
        0x0040_0005,
        Duration::from_secs(2),
        RESTART_REPRODUCE,
    );
    let result = async {
        let original = SpaceAdmissionProtocolTestPair::receiving_complete().await;
        original
            .joiner()
            .start_join_at(join_input("restart-persisted"), 1_000)
            .await
            .map_err(|_| fixture("join-start"))?;
        for _ in 0..3 {
            original
                .joiner()
                .recover_pending(AdmissionRecoveryTrigger::StateChanged)
                .await;
        }
        original
            .joiner()
            .complete_pending_space_transition()
            .await
            .map_err(|_| fixture("activation"))?;
        let persisted = original.saved_join();
        require(
            persisted.is_active()
                && persisted.pending_exchange().is_some_and(|exchange| {
                    exchange.request_envelope().kind() == SpaceAdmissionMessageKind::CompleteAck
                }),
            "restart-checkpoint-was-not-pending",
        )?;
        drop(original);
        scenario.record_event("application-stopped");

        let rebuilt = SpaceAdmissionProtocolTestPair::reopen_receiving_complete(persisted).await;
        rebuilt
            .joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
        scenario.record_event("rebuilt-application-settled");
        require(
            rebuilt.saved_join().is_active_settled(),
            "rebuilt-application-did-not-resume-persisted-admission",
        )
    }
    .await;
    finish(scenario, result);
}

fn join_input(code: &str) -> JoinSpaceInput {
    JoinSpaceInput {
        invitation_code: uc_core::pairing::InvitationCode::new(code),
        device_name: Some("Scenario device".to_owned()),
        passphrase: uc_core::crypto::domain::Passphrase::new("scenario-passphrase"),
        preserve_unreadable_history: false,
    }
}

fn fixture(condition: &'static str) -> uc_testkit::ScenarioFailure {
    uc_testkit::ScenarioFailure::new(uc_testkit::FailureKind::FixtureInvalid, condition)
}
