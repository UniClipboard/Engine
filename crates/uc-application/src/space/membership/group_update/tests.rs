use std::sync::Arc;

use async_trait::async_trait;
use uc_core::membership::GroupEpoch;

use super::{
    ApplyGroupUpdatePort, GroupUpdateApplyCoordinator, GroupUpdateApplyError,
    VerifiedHistoryGroupUpdate,
};
use crate::connectivity::ConnectionLease;
use crate::space::membership::testing::{
    EstablishedSpace, NoopAddressRefresh, OfflineHistoryExchange, OwnerFixture, RecordingEffects,
};
use crate::space::membership::HistorySynchronizer;

struct UnusedApply;
#[async_trait]
impl ApplyGroupUpdatePort for UnusedApply {
    async fn apply(
        &self,
        _: &[u8],
        _: Option<&VerifiedHistoryGroupUpdate>,
        _: Option<&ConnectionLease>,
    ) -> Result<GroupEpoch, GroupUpdateApplyError> {
        panic!("proof lookup must not apply security state")
    }
}

#[tokio::test]
async fn activation_baseline_is_a_trusted_boundary_without_a_regular_event() {
    let space = EstablishedSpace::new(&["device-a", "device-b"]);
    let owner = OwnerFixture::new(space.record("device-a", 1)).owner;
    let history = Arc::new(HistorySynchronizer::new(
        owner.clone(),
        Arc::new(OfflineHistoryExchange),
        Arc::new(NoopAddressRefresh),
    ));
    let coordinator = GroupUpdateApplyCoordinator::new(
        owner,
        Arc::new(UnusedApply),
        Arc::new(RecordingEffects::default()),
        history,
    );
    assert!(coordinator
        .proof(b"untrusted incoming update")
        .await
        .unwrap()
        .is_none());
    assert!(
        coordinator.proof(&[]).await.unwrap().is_none(),
        "baseline must not invent payload evidence"
    );
}
