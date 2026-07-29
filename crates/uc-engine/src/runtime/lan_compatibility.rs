use super::{engine_event_for_mobile_settings_update, ProductionRuntime};
use crate::compatibility::mobile_lan::content_operations::{
    execute_apply_mobile_sync_document, execute_check_mobile_content_available,
    execute_query_latest_mobile_sync_document, execute_read_mobile_sync_file,
};
use crate::compatibility::mobile_lan::operations::{
    execute_authenticate_mobile_request, execute_list_mobile_devices,
    execute_query_mobile_sync_settings, execute_register_mobile_device,
    execute_revalidate_mobile_credential, execute_revoke_mobile_device,
    execute_update_mobile_device, execute_update_mobile_sync_settings,
};
use crate::{EngineError, EngineErrorCategory, Operation, OperationResult};

impl ProductionRuntime {
    pub(super) async fn execute_lan_compatibility_operation(
        &self,
        operation: Operation,
    ) -> Result<OperationResult, EngineError> {
        match operation {
            Operation::ListMobileDevices => {
                execute_list_mobile_devices(self.current_mobile_sync().await?.as_ref()).await
            }
            Operation::RevokeMobileDevice(input) => {
                execute_revoke_mobile_device(self.current_mobile_sync().await?.as_ref(), input)
                    .await
            }
            Operation::AuthenticateMobileRequest(input) => {
                execute_authenticate_mobile_request(
                    self.current_mobile_sync().await?.as_ref(),
                    input,
                )
                .await
            }
            Operation::RevalidateMobileCredential(input) => {
                execute_revalidate_mobile_credential(
                    self.current_mobile_sync().await?.as_ref(),
                    input,
                )
                .await
            }
            Operation::ListMobileLanInterfaces => {
                let interfaces = self
                    .current_mobile_sync()
                    .await?
                    .list_lan_interfaces()
                    .await
                    .map_err(|_| EngineError::new(1450, EngineErrorCategory::Unavailable, true))?;
                Ok(OperationResult::MobileLanInterfaces(
                    interfaces
                        .into_iter()
                        .map(|interface| crate::MobileLanInterfaceSummary {
                            name: interface.name,
                            ipv4: interface.ipv4,
                        })
                        .collect(),
                ))
            }
            Operation::QueryMobileSyncSettings => {
                execute_query_mobile_sync_settings(self.current_mobile_sync().await?.as_ref()).await
            }
            Operation::UpdateMobileSyncSettings(patch) => {
                let result = execute_update_mobile_sync_settings(
                    self.current_mobile_sync().await?.as_ref(),
                    *patch,
                )
                .await;
                if let Ok(OperationResult::MobileSyncSettingsUpdated(
                    crate::MobileSyncSettingsUpdateOutcome::Updated(settings),
                )) = &result
                {
                    self.events
                        .send(engine_event_for_mobile_settings_update(settings));
                }
                result
            }
            Operation::UpdateMobileLanEndpoint(update) => {
                self.mobile_lan_endpoint.update(update).await
            }
            Operation::RegisterMobileDevice(input) => {
                execute_register_mobile_device(self.current_mobile_sync().await?.as_ref(), input)
                    .await
            }
            Operation::UpdateMobileDevice(input) => {
                execute_update_mobile_device(self.current_mobile_sync().await?.as_ref(), input)
                    .await
            }
            Operation::CheckMobileContentAvailable(input) => {
                execute_check_mobile_content_available(
                    self.current_mobile_sync().await?.as_ref(),
                    input,
                )
                .await
            }
            Operation::QueryLatestMobileSyncDocument => {
                execute_query_latest_mobile_sync_document(
                    self.current_mobile_sync().await?.as_ref(),
                )
                .await
            }
            Operation::ApplyMobileSyncDocument(input) => {
                execute_apply_mobile_sync_document(
                    self.current_mobile_sync().await?.as_ref(),
                    *input,
                )
                .await
            }
            Operation::ReadMobileSyncFile(input) => {
                execute_read_mobile_sync_file(self.current_mobile_sync().await?.as_ref(), input)
                    .await
            }
            Operation::BeginMobileFileUpload(input) => self.begin_mobile_file_upload(input).await,
            Operation::AppendMobileFileUpload(input) => self.append_mobile_file_upload(input).await,
            Operation::FinishMobileFileUpload(input) => self.finish_mobile_file_upload(input).await,
            Operation::AbortMobileFileUpload(input) => {
                self.abort_mobile_file_upload(input.handle).await
            }
            _ => Err(super::operation_unavailable_error()),
        }
    }
}
