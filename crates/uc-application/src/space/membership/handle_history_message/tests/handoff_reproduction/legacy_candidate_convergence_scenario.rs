use std::time::Duration;

use uc_core::membership::{
    MembershipConflictEvidenceV3, MembershipHistoryAckV3, MembershipHistoryMessage,
};

use super::*;
use crate::test_support::membership_scenario::{finish, require, scenario};

const REPRODUCE: &str =
    "cargo nextest run -p uc-application -E 'test(legacy_candidates_converge_by_evidence)'";

#[tokio::test]
async fn legacy_candidates_converge_by_evidence() {
    let scenario = scenario(
        "legacy-candidate-convergence",
        0x0040_0002,
        Duration::from_secs(1),
        REPRODUCE,
    );
    let result = async {
        let completed = Fixture::new();
        completed.deliver(&completed.peer).await;
        completed.keep_local().await;
        completed.deliver(&completed.peer).await;
        completed.assert_no_prompt().await;
        scenario.record_event("legacy-candidate-completed");

        let incomplete = Fixture::new();
        incomplete.deliver(&incomplete.peer).await;
        let incomplete_view = incomplete
            .resolver()
            .query()
            .await
            .map_err(|_| fixture("query-incomplete"))?;
        require(
            incomplete_view
                .conflicts
                .iter()
                .filter(|conflict| !conflict.local_resolution_completed)
                .count()
                == 1,
            "provably-incomplete-candidate-did-not-require-action",
        )?;
        scenario.record_event("legacy-candidate-provably-incomplete");

        let insufficient = Fixture::new();
        let response = HandleMembershipHistoryMessageUseCase::new(insufficient.ledger.clone())
            .execute(
                &AuthenticatedMember::new(insufficient.peer.device_id.clone()),
                MembershipHistoryMessage::ConflictEvidenceV3(MembershipConflictEvidenceV3 {
                    transfer_id: [0; 32],
                    pages: insufficient
                        .remote
                        .export_conflict_evidence_pages_v2(insufficient.peer.clone())
                        .map_err(|_| fixture("invalid-evidence-fixture"))?,
                }),
            )
            .await
            .map_err(|_| fixture("deliver-insufficient"))?;
        require(
            response == MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid),
            "insufficient-evidence-was-promoted",
        )?;
        require(
            insufficient
                .repository
                .load()
                .await
                .map_err(|_| fixture("load-insufficient"))?
                .membership_conflicts
                .is_empty(),
            "insufficient-evidence-created-a-candidate",
        )?;
        scenario.record_event("legacy-candidate-evidence-insufficient");
        Ok(())
    }
    .await;
    finish(scenario, result);
}

fn fixture(condition: &'static str) -> uc_testkit::ScenarioFailure {
    uc_testkit::ScenarioFailure::new(uc_testkit::FailureKind::FixtureInvalid, condition)
}
