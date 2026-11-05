//! 成员同步偏好的输入校验与对外投影。

use super::*;

pub(super) fn validate_device_id(device_id: &str) -> Result<(), EngineError> {
    if device_id.trim().is_empty() {
        return Err(EngineError::new(
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
        ));
    }
    Ok(())
}

pub(super) fn into_app_patch(patch: MemberSyncPreferencesPatch) -> AppMemberSyncPreferencesPatch {
    AppMemberSyncPreferencesPatch {
        send_enabled: patch.send_enabled,
        receive_enabled: patch.receive_enabled,
        send_content_types: patch.send_content_types.map(into_app_content_types_patch),
        receive_content_types: patch
            .receive_content_types
            .map(into_app_content_types_patch),
    }
}

pub(super) fn into_app_content_types_patch(patch: ContentTypesPatch) -> AppContentTypesPatch {
    AppContentTypesPatch {
        text: patch.text,
        image: patch.image,
        link: patch.link,
        file: patch.file,
        code_snippet: patch.code_snippet,
        rich_text: patch.rich_text,
    }
}

pub(super) fn member_preferences_result(preferences: MemberSyncPreferencesView) -> OperationResult {
    OperationResult::MemberSyncPreferences(MemberSyncPreferencesSummary {
        send_enabled: preferences.send_enabled,
        receive_enabled: preferences.receive_enabled,
        send_content_types: ContentTypesSummary {
            text: preferences.send_content_types.text,
            image: preferences.send_content_types.image,
            link: preferences.send_content_types.link,
            file: preferences.send_content_types.file,
            code_snippet: preferences.send_content_types.code_snippet,
            rich_text: preferences.send_content_types.rich_text,
        },
        receive_content_types: ContentTypesSummary {
            text: preferences.receive_content_types.text,
            image: preferences.receive_content_types.image,
            link: preferences.receive_content_types.link,
            file: preferences.receive_content_types.file,
            code_snippet: preferences.receive_content_types.code_snippet,
            rich_text: preferences.receive_content_types.rich_text,
        },
    })
}

pub(super) fn map_roster_error(error: RosterError) -> EngineError {
    let (code, category, retryable, variant) = match error {
        RosterError::MembershipReconciliationUnavailable { .. } => (
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
            "workspace_convergence_unavailable",
        ),
        RosterError::MembershipReconciliationLocked => (
            QUERY_WORKSPACE_CONVERGENCE_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            false,
            "workspace_convergence_locked",
        ),
        RosterError::MembershipReconciliationCorrupt => (
            QUERY_WORKSPACE_CONVERGENCE_CORRUPT_CODE,
            EngineErrorCategory::InvalidState,
            false,
            "workspace_convergence_corrupt",
        ),
        RosterError::MemberRemoval(_) => (
            QUERY_WORKSPACE_CONVERGENCE_FAILED_CODE,
            EngineErrorCategory::InvalidState,
            false,
            "workspace_convergence_failed",
        ),
        RosterError::MemberRemovalInvalidInput => (
            MEMBER_INVALID_INPUT_CODE,
            EngineErrorCategory::InvalidInput,
            false,
            "member_removal_invalid_input",
        ),
        RosterError::MemberRemovalTargetNotFound => (
            MEMBER_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
            "member_removal_target_not_found",
        ),
        RosterError::NotFound(_) => (
            MEMBER_NOT_FOUND_CODE,
            EngineErrorCategory::NotFound,
            false,
            "not_found",
        ),
        RosterError::Unavailable => (
            MEMBER_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
            "unavailable",
        ),
        RosterError::MemberRepository(_) => (
            MEMBER_REPOSITORY_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
            "member_repository",
        ),
        RosterError::LocalIdentity(_) => (
            MEMBER_LOCAL_IDENTITY_FAILED_CODE,
            EngineErrorCategory::Internal,
            false,
            "local_identity",
        ),
        RosterError::GroupBootstrap(_) => (
            MEMBER_LEGACY_BOOTSTRAP_FAILED_CODE,
            EngineErrorCategory::Internal,
            true,
            "legacy_bootstrap",
        ),
        RosterError::SpaceProtection(_) => (
            SPACE_PROTECTION_FAILED_CODE,
            EngineErrorCategory::Internal,
            true,
            "space_protection",
        ),
    };
    uc_error!(
        operation = "member_roster",
        variant = variant,
        error_code = code,
        error_category = log_vocab(&category),
        retryable = retryable,
        "member roster operation failed"
    );
    EngineError::new(code, category, retryable)
}

#[cfg(test)]
mod tests {
    use super::*;

    use uc_core::membership::MembershipError;

    #[test]
    fn roster_failures_keep_stable_categories_and_distinct_codes() {
        let missing = map_roster_error(RosterError::NotFound("private id".into()));
        let unavailable = map_roster_error(RosterError::Unavailable);
        let repository = map_roster_error(RosterError::MemberRepository(
            MembershipError::Repository("private detail".into()),
        ));

        assert_eq!(missing.category(), EngineErrorCategory::NotFound);
        assert_eq!(unavailable.category(), EngineErrorCategory::Unavailable);
        assert_eq!(repository.category(), EngineErrorCategory::Internal);
        assert_ne!(missing.code(), unavailable.code());
        assert_ne!(unavailable.code(), repository.code());
    }
}
