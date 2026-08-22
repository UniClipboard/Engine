//! Shared local-device query operation.

use uc_application::facade::AppFacade;

use crate::{EngineError, LocalDeviceSummary, OperationResult};

pub async fn execute_query_local_device(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let info = facade.local_device_info().await;

    Ok(OperationResult::LocalDevice(LocalDeviceSummary {
        device_id: info.device_id,
        display_name: info.device_name,
    }))
}
