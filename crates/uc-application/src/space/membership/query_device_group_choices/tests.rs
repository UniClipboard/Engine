use std::sync::Arc;

use async_trait::async_trait;

use super::QueryDeviceGroupChoicesUseCase;
use crate::space::admission::CurrentJoinStatus;
use crate::space::membership::testing::OwnerFixture;
use crate::space::membership::{
    DeviceTrustObservation, LoadCurrentJoinStatusPort, LoadDeviceTrustObservationsPort,
    MembershipRecord, QueryDeviceTrustError, QueryDeviceTrustUseCase,
    ResolveMembershipConflictUseCase,
};

struct EmptyInputs;

#[async_trait]
impl LoadDeviceTrustObservationsPort for EmptyInputs {
    async fn load(
        &self,
        _: &[uc_core::DeviceId],
    ) -> Result<Vec<DeviceTrustObservation>, QueryDeviceTrustError> {
        panic!("empty space has no devices");
    }
}

#[async_trait]
impl LoadCurrentJoinStatusPort for EmptyInputs {
    async fn load_current_join(&self) -> Result<Option<CurrentJoinStatus>, QueryDeviceTrustError> {
        Ok(None)
    }
}

#[tokio::test]
async fn one_device_group_query_reads_one_membership_view() {
    let fixture = OwnerFixture::new(MembershipRecord::NoSpace { revision: 5 });
    let trust = Arc::new(QueryDeviceTrustUseCase::new_for_tests(
        fixture.owner.clone(),
        Arc::new(EmptyInputs),
        Arc::new(EmptyInputs),
    ));
    let conflicts = Arc::new(ResolveMembershipConflictUseCase::new(
        fixture.owner.clone(),
        trust.clone(),
    ));
    let query = QueryDeviceGroupChoicesUseCase::new(fixture.owner.clone(), trust, conflicts);

    let view = query
        .execute()
        .await
        .expect("one query must return one coherent membership view");

    assert_eq!(view.revision, 5);
    assert_eq!(view.revision, view.device_trust.revision);
    assert_eq!(view.revision, view.conflicts.revision);
    assert_eq!(fixture.records.load_count(), 1);
}
