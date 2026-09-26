use std::time::Duration;

use super::*;
use crate::test_support::membership_scenario::{finish, require, scenario};

const REPRODUCE: &str = "cargo nextest run -p uc-application -E 'test(device_state_update_retries_requires_attention_and_recovers)'";

#[tokio::test]
async fn device_state_update_retries_requires_attention_and_recovers() {
    let scenario = scenario(
        "device-state-retry-attention-recovery",
        0x0040_0004,
        Duration::from_secs(2),
        REPRODUCE,
    );
    let result = async {
        let (owner, signer, change_id) = pending_local_removal();
        owner.records.conflict_next_commits(1);
        let (decide, query) = decide_case(&owner, signer, Arc::new(NoopEffects));

        let attention = decide
            .execute(DecideDeviceTrustChange {
                change_id,
                choice: DeviceTrustChangeChoice::ApplyChange,
                confirm_local_removal: false,
            })
            .await
            .map_err(|_| fixture("attention-query"))?;
        require(
            matches!(
                attention,
                DecideDeviceTrustChangeResult::LocalConfirmationRequired { .. }
            ),
            "device-state-did-not-require-local-attention",
        )?;
        scenario.record_event("device-state-needs-attention");

        let recovered = decide
            .execute(DecideDeviceTrustChange {
                change_id,
                choice: DeviceTrustChangeChoice::KeepCurrentDeviceGroup,
                confirm_local_removal: false,
            })
            .await
            .map_err(|_| fixture("retry-recovery"))?;
        require(
            matches!(
                recovered,
                DecideDeviceTrustChangeResult::KeptCurrentDeviceGroup { .. }
            ),
            "retryable-device-state-update-did-not-recover",
        )?;
        require(
            owner.records.commit_count() == 1,
            "device-state-retry-did-not-commit-once",
        )?;
        require(
            query
                .execute()
                .await
                .map_err(|_| fixture("final-status"))?
                .current_change
                .is_none(),
            "device-state-remained-actionable-after-recovery",
        )?;
        scenario.record_event("device-state-recovered");
        Ok(())
    }
    .await;
    finish(scenario, result);
}

fn fixture(condition: &'static str) -> uc_testkit::ScenarioFailure {
    uc_testkit::ScenarioFailure::new(uc_testkit::FailureKind::FixtureInvalid, condition)
}
