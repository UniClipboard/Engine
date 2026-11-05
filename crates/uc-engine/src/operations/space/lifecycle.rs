//! 空间自生命周期动作：创建、加入、取消加入、重置、出厂重置与设置状态查询。
//!
//! 这些动作共同拥有“本机空间处于哪个生命周期阶段”的对外结果，因此合并在同一能力簇；
//! 各自的错误码契约不同，映射函数保持一一对应。

use std::error::Error;

use base64::Engine as _;
use uc_application::facade::{
    AppFacade, CancelSpaceJoinError, InitializeSpaceError,
    InitializeSpaceInput as AppInitializeSpaceInput, JoinSpaceError as AppJoinSpaceError,
    JoinSpaceInput as AppJoinSpaceInput, ProfileFactoryResetError, ProfileFactoryResetFacade,
    ProfileFactoryResetRequest, QuerySetupStateError, ResetSpaceError,
};
use uc_core::crypto::domain::Passphrase;
use uc_core::pairing::InvitationCode;
use uc_observability_contract::{error_source::io_error_kind, uc_error, uc_info};

use crate::error_codes::*;
use crate::operations::device::member::join_space_status;
use crate::{
    CancelJoinSpaceInput, CreateSpaceInput, EngineError, EngineErrorCategory, JoinSpaceInput,
    OperationResult, SetupInvitationSummary, SetupStateSummary,
};

pub async fn execute_create_space(
    facade: &AppFacade,
    input: CreateSpaceInput,
) -> Result<OperationResult, EngineError> {
    let result = facade
        .initialize_space(AppInitializeSpaceInput {
            passphrase: Passphrase::new(input.passphrase.expose()),
            passphrase_confirm: Passphrase::new(input.passphrase_confirmation.expose()),
            device_name: input.device_name,
        })
        .await
        .map_err(map_create_space_error)?;

    Ok(OperationResult::SpaceCreated {
        space_id: result.space_id.as_ref().to_string(),
        self_device_id: result.self_device_id.to_string(),
        identity_fingerprint: result.fingerprint.as_display().to_string(),
    })
}

fn map_create_space_error(error: InitializeSpaceError) -> EngineError {
    match error {
        InitializeSpaceError::PassphraseMismatch => EngineError::new(
            CREATE_SPACE_PASSPHRASE_MISMATCH_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        InitializeSpaceError::DeviceNameRequired => EngineError::new(
            CREATE_SPACE_DEVICE_NAME_REQUIRED_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ),
        InitializeSpaceError::AlreadyInitialized => EngineError::new(
            CREATE_SPACE_ALREADY_INITIALIZED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        InitializeSpaceError::AlreadySetup => EngineError::new(
            CREATE_SPACE_ALREADY_SETUP_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        InitializeSpaceError::StorageFailed { .. } | InitializeSpaceError::Internal { .. } => {
            uc_error!(
                error_kind = "create_space",
                io_error_kind = io_error_kind(&error),
                "create space failed"
            );
            EngineError::new(
                CREATE_SPACE_FAILED_CODE,
                EngineErrorCategory::Internal,
                false,
            )
        }
    }
}

pub async fn execute_join_space(
    facade: &AppFacade,
    input: JoinSpaceInput,
    started_at_ms: i64,
) -> Result<OperationResult, EngineError> {
    let joined = facade
        .join_space(
            AppJoinSpaceInput {
                invitation_code: InvitationCode::new(input.invitation_code),
                device_name: input.device_name,
                passphrase: Passphrase::new(input.passphrase.expose()),
                preserve_unreadable_history: input.preserve_unreadable_history,
            },
            started_at_ms,
        )
        .await
        .map_err(map_join_space_error)?;
    Ok(join_status_result(joined.status))
}

pub(crate) fn join_status_result(
    status: uc_application::facade::CurrentJoinStatus,
) -> OperationResult {
    OperationResult::JoinSpace(join_space_status(status))
}

fn map_join_space_error(error: AppJoinSpaceError) -> EngineError {
    match error {
        AppJoinSpaceError::DeviceNameRequired => device_name_required_error(),
        AppJoinSpaceError::InvalidInvitation => error_with(
            JOIN_SPACE_INVITATION_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
        ),
        AppJoinSpaceError::PreviousJoinCannotBeSuperseded { .. } => error_with(
            JOIN_SPACE_PREVIOUS_JOIN_CANNOT_BE_SUPERSEDED_CODE,
            EngineErrorCategory::Conflict,
            false,
        ),
        AppJoinSpaceError::Locked => error_with(
            JOIN_SPACE_NOT_UNLOCKED_CODE,
            EngineErrorCategory::Unauthorized,
            false,
        ),
        AppJoinSpaceError::StateChanged => {
            error_with(JOIN_SPACE_STORAGE_CODE, EngineErrorCategory::Conflict, true)
        }
        AppJoinSpaceError::RecoveryRequired => error_with(
            JOIN_SPACE_PENDING_MIGRATION_CODE,
            EngineErrorCategory::InvalidState,
            false,
        ),
        AppJoinSpaceError::Unavailable => unavailable_error(JOIN_SPACE_STORAGE_CODE),
        AppJoinSpaceError::Settings(_) | AppJoinSpaceError::InvalidStartMaterial { .. } => {
            join_internal_error("join space", error)
        }
    }
}

pub async fn execute_cancel_join_space(
    facade: &AppFacade,
    input: CancelJoinSpaceInput,
) -> Result<OperationResult, EngineError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(input.join_id)
        // discarded-source[input-validation]: `base64::DecodeError`: the rejection reason is fully expressed by the target classification
        .map_err(|_| not_found())?;
    // discarded-source[no-information]: the error value carries no usable diagnostic information
    let join_id: [u8; 16] = bytes.try_into().map_err(|_| not_found())?;
    facade
        .cancel_space_join(join_id)
        .await
        .map(|status| OperationResult::JoinSpace(join_space_status(status)))
        .map_err(map_cancel_join_error)
}

fn map_cancel_join_error(error: CancelSpaceJoinError) -> EngineError {
    match error {
        CancelSpaceJoinError::NotFound => not_found(),
        // 失败记录由取消加入的流程负责人写；这里只做稳定错误码映射。
        CancelSpaceJoinError::State { .. } => {
            EngineError::new(JOIN_SPACE_FAILED_CODE, EngineErrorCategory::Internal, false)
        }
    }
}

fn not_found() -> EngineError {
    EngineError::new(
        CANCEL_JOIN_SPACE_NOT_FOUND_CODE,
        EngineErrorCategory::NotFound,
        false,
    )
}

pub async fn execute_reset_space(facade: &AppFacade) -> Result<OperationResult, EngineError> {
    facade.reset_space().await.map_err(|error| match error {
        ResetSpaceError::PreparationFailed(_)
        | ResetSpaceError::StagingFailed(_)
        | ResetSpaceError::RebuildFailed(_)
        | ResetSpaceError::CommitFailed(_)
        | ResetSpaceError::FinalizationFailed(_)
        | ResetSpaceError::Internal(_) => {
            uc_error!(
                error_kind = "reset_space",
                io_error_kind = io_error_kind(&error),
                "reset space failed"
            );
            EngineError::new(
                RESET_SPACE_FAILED_CODE,
                EngineErrorCategory::Internal,
                false,
            )
        }
    })?;
    Ok(OperationResult::SpaceReset)
}

pub async fn execute_factory_reset_space(
    reset: &ProfileFactoryResetFacade,
) -> Result<OperationResult, EngineError> {
    reset
        .execute(ProfileFactoryResetRequest::Start)
        .await
        .map_err(map_profile_factory_reset_error)?;
    Ok(OperationResult::SpaceFactoryReset)
}

pub(crate) fn map_profile_factory_reset_error(error: ProfileFactoryResetError) -> EngineError {
    let code = match error {
        ProfileFactoryResetError::StopRuntime { .. } => FACTORY_RESET_UNAVAILABLE_CODE,
        ProfileFactoryResetError::WipeKeys { .. }
        | ProfileFactoryResetError::RetireUpgradeBackupSecurity { .. } => {
            FACTORY_RESET_KEY_MATERIAL_FAILED_CODE
        }
        ProfileFactoryResetError::ClearState { .. } => FACTORY_RESET_STORAGE_FAILED_CODE,
        ProfileFactoryResetError::Lifecycle(_)
        | ProfileFactoryResetError::Repository(_)
        | ProfileFactoryResetError::LifecycleMissing => FACTORY_RESET_FAILED_CODE,
    };
    uc_error!(
        code = code,
        error_kind = "factory_reset",
        io_error_kind = io_error_kind(&error),
        "factory reset space failed"
    );
    EngineError::new(code, EngineErrorCategory::Internal, false)
}

pub async fn execute_query_setup_state(facade: &AppFacade) -> Result<OperationResult, EngineError> {
    let state = facade
        .query_setup_state()
        .await
        .map_err(|error| match error {
            QuerySetupStateError::StorageFailed(_) | QuerySetupStateError::Internal(_) => {
                let source = match error {
                    QuerySetupStateError::StorageFailed(_) => "storage",
                    QuerySetupStateError::Internal(_) => "internal",
                };
                uc_error!(
                    operation = "query_setup_state",
                    source = source,
                    error_code = QUERY_SETUP_STATE_FAILED_CODE,
                    error_category = "internal",
                    retryable = false,
                    "space state query failed"
                );
                EngineError::new(
                    QUERY_SETUP_STATE_FAILED_CODE,
                    EngineErrorCategory::Internal,
                    false,
                )
            }
        })?;

    let summary = SetupStateSummary {
        has_completed: state.has_completed,
        space_id: state.space_id.map(Into::into),
        current_invitation: state
            .current_invitation
            .map(|invitation| SetupInvitationSummary {
                invitation_code: invitation.code.as_str().to_string(),
                full_invitation: invitation.full_invitation.into_string(),
                expires_at_ms: invitation.expires_at.timestamp_millis(),
            }),
        device_name: state.device_name,
        re_pairing_required: state.re_pairing_required,
    };
    uc_info!(
        operation = "query_setup_state",
        has_completed = summary.has_completed,
        has_space = summary.space_id.is_some(),
        has_current_invitation = summary.current_invitation.is_some(),
        has_device_name = summary.device_name.is_some(),
        "space state query completed"
    );
    Ok(OperationResult::SetupState(summary))
}

fn device_name_required_error() -> EngineError {
    error_with(
        JOIN_SPACE_DEVICE_NAME_REQUIRED_CODE,
        EngineErrorCategory::InvalidInput,
        false,
    )
}

fn unavailable_error(code: u32) -> EngineError {
    error_with(code, EngineErrorCategory::Unavailable, true)
}

fn error_with(code: u32, category: EngineErrorCategory, retryable: bool) -> EngineError {
    EngineError::new(code, category, retryable)
}

fn join_internal_error(
    context: &'static str,
    error: impl Into<Box<dyn Error + Send + Sync>>,
) -> EngineError {
    let error = error.into();
    uc_error!(
        context = context,
        io_error_kind = io_error_kind(error.as_ref()),
        "join-space operation failed"
    );
    error_with(JOIN_SPACE_FAILED_CODE, EngineErrorCategory::Internal, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_and_conflict_failures_keep_distinct_stable_codes() {
        let passphrase = map_create_space_error(InitializeSpaceError::PassphraseMismatch);
        let device_name = map_create_space_error(InitializeSpaceError::DeviceNameRequired);
        let initialized = map_create_space_error(InitializeSpaceError::AlreadyInitialized);
        let setup = map_create_space_error(InitializeSpaceError::AlreadySetup);

        assert_ne!(passphrase.code(), device_name.code());
        assert_ne!(initialized.code(), setup.code());
    }

    #[test]
    fn join_local_start_failures_keep_stable_product_categories() {
        let locked = map_join_space_error(AppJoinSpaceError::Locked);
        let changed = map_join_space_error(AppJoinSpaceError::StateChanged);
        let recovery = map_join_space_error(AppJoinSpaceError::RecoveryRequired);
        let unavailable = map_join_space_error(AppJoinSpaceError::Unavailable);

        assert_eq!(locked.code(), JOIN_SPACE_NOT_UNLOCKED_CODE);
        assert_eq!(locked.category(), EngineErrorCategory::Unauthorized);
        assert_eq!(changed.category(), EngineErrorCategory::Conflict);
        assert!(changed.is_retryable());
        assert_eq!(recovery.category(), EngineErrorCategory::InvalidState);
        assert_eq!(unavailable.category(), EngineErrorCategory::Unavailable);
        assert!(unavailable.is_retryable());
    }

    #[test]
    fn previous_join_cannot_be_superseded_is_a_stable_conflict() {
        let error = map_join_space_error(AppJoinSpaceError::previous_join_cannot_be_superseded());

        assert_eq!(
            error.code(),
            JOIN_SPACE_PREVIOUS_JOIN_CANNOT_BE_SUPERSEDED_CODE
        );
        assert_eq!(error.category(), EngineErrorCategory::Conflict);
        assert!(!error.is_retryable());
    }

    #[test]
    fn cancel_join_state_failure_maps_to_a_stable_code_without_a_second_record() {
        let logs = uc_testkit::log_capture::CapturedLogs::default();
        let _guard = logs.install();

        let missing = map_cancel_join_error(CancelSpaceJoinError::NotFound);
        let failed = map_cancel_join_error(CancelSpaceJoinError::State {
            source: anyhow::Error::new(std::io::Error::other("PRIVATE_STATE")),
        });

        assert_eq!(missing.code(), CANCEL_JOIN_SPACE_NOT_FOUND_CODE);
        assert_eq!(failed.code(), JOIN_SPACE_FAILED_CODE);
        assert_eq!(logs.count("cancel"), 0, "{}", logs.output());
    }

    #[test]
    fn factory_reset_failures_keep_distinct_stable_codes() {
        let key_material = map_profile_factory_reset_error(ProfileFactoryResetError::wipe_keys());
        let storage = map_profile_factory_reset_error(ProfileFactoryResetError::clear_state());
        let internal = map_profile_factory_reset_error(ProfileFactoryResetError::Repository(
            uc_application::deps::ProfileLifecycleRepositoryError::corrupt(),
        ));

        assert_ne!(key_material.code(), storage.code());
        assert_ne!(storage.code(), internal.code());
        assert_ne!(key_material.code(), internal.code());
    }
}
