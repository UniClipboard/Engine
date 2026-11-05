//! 自定义中继的查询、增删改与探测。

use std::error::Error;

use uc_application::facade::settings as app;
use uc_application::facade::AppFacade;
use uc_core::settings::relay_routing::RelayRouting;

use super::settings::{internal_error, map_patch, map_settings};
use crate::error_codes::*;
use crate::{
    CustomRelayMutation, CustomRelayMutationOutcome, CustomRelayRejection, CustomRelaySummary,
    EngineError, EngineErrorCategory, OperationResult, RelayCredentialEdit, RelayCredentialInput,
    RelayCredentialStatus, RelayEntrySource, RelayOverview, RelayOverviewEntry,
    RelayProbeCredential, RelayProbeInput, RelayProbeOutcome, RelayRoutingMode, SaveRelayInput,
    SaveRelayOutcome,
};

pub(crate) async fn execute_query_custom_relays(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let relays = facade.list_relays().await.map_err(map_save_relay_error)?;
    Ok(OperationResult::CustomRelays(
        relays.into_iter().map(map_custom_relay).collect(),
    ))
}

pub(crate) async fn execute_query_relay_overview(
    facade: &AppFacade,
) -> Result<OperationResult, EngineError> {
    let overview = facade
        .relay_overview()
        .await
        .map_err(map_save_relay_error)?;
    Ok(OperationResult::RelayOverview(RelayOverview {
        saved_mode: map_routing_mode(overview.saved_routing),
        applied_mode: overview.applied_routing.map(map_routing_mode),
        change_pending: overview.change_pending,
        entries: overview
            .entries
            .into_iter()
            .map(|entry| RelayOverviewEntry {
                source: match entry.source {
                    app::RelayEntrySource::BuiltIn => RelayEntrySource::BuiltIn,
                    app::RelayEntrySource::Custom => RelayEntrySource::Custom,
                },
                region_id: entry.region.map(str::to_owned),
                url: entry.url,
                credential_configured: entry.credential_configured,
                in_effect: entry.in_effect,
            })
            .collect(),
    }))
}

pub(crate) async fn execute_mutate_custom_relay(
    facade: &AppFacade,
    mutation: CustomRelayMutation,
) -> Result<OperationResult, EngineError> {
    let mutation = map_custom_relay_mutation(mutation)?;
    let outcome = match facade
        .mutate_relays(mutation)
        .await
        .map_err(map_save_relay_error)?
    {
        Ok(relays) => CustomRelayMutationOutcome::Saved {
            relays: relays.into_iter().map(map_custom_relay).collect(),
        },
        Err(reason) => CustomRelayMutationOutcome::Rejected {
            reason: match reason {
                app::RelayConfigurationRejection::InvalidUrl => CustomRelayRejection::InvalidUrl,
                app::RelayConfigurationRejection::Duplicate => CustomRelayRejection::Duplicate,
                app::RelayConfigurationRejection::NotFound => CustomRelayRejection::NotFound,
            },
        },
    };
    Ok(OperationResult::CustomRelayMutated(outcome))
}

fn map_routing_mode(routing: RelayRouting) -> RelayRoutingMode {
    match routing {
        RelayRouting::BuiltIn => RelayRoutingMode::BuiltIn,
        RelayRouting::Custom => RelayRoutingMode::Custom,
        RelayRouting::Disabled => RelayRoutingMode::Disabled,
    }
}

fn map_custom_relay_mutation(
    mutation: CustomRelayMutation,
) -> Result<app::RelayConfigurationMutation, EngineError> {
    Ok(match mutation {
        CustomRelayMutation::Add { url, access_token } => app::RelayConfigurationMutation::Add {
            url,
            access_token: access_token
                .map(|token| app::RelayAccessToken::new(token.expose().to_string()))
                .transpose()
                // discarded-source[input-validation]: the rejection reason is fully expressed by the target classification
                .map_err(|_| invalid_relay_token_error())?,
        },
        CustomRelayMutation::Edit {
            previous_url,
            url,
            access_token,
        } => app::RelayConfigurationMutation::Edit {
            previous_url,
            url,
            access_token: access_token
                .map(|token| app::RelayAccessToken::new(token.expose().to_string()))
                .transpose()
                // discarded-source[input-validation]: the rejection reason is fully expressed by the target classification
                .map_err(|_| invalid_relay_token_error())?,
        },
        CustomRelayMutation::Delete { url } => app::RelayConfigurationMutation::Delete { url },
    })
}

fn invalid_relay_token_error() -> EngineError {
    map_relay_credential_error(
        app::SettingsFacadeError::RelayCredentialInvalidToken,
        SAVE_RELAY_FAILED_CODE,
    )
}

fn map_custom_relay(entry: app::RelayConfigurationEntry) -> CustomRelaySummary {
    CustomRelaySummary {
        url: entry.url,
        credential_configured: entry.credential_configured,
    }
}

fn relay_probe_host_message(source: &(dyn Error + Send + Sync)) -> String {
    source.to_string()
}

pub(crate) async fn execute_probe_relay(
    facade: &AppFacade,
    input: RelayProbeInput,
) -> Result<OperationResult, EngineError> {
    let credential = match input.credential {
        RelayProbeCredential::Stored => app::RelayProbeCredential::Stored,
        RelayProbeCredential::None => app::RelayProbeCredential::None,
        RelayProbeCredential::Override(token) => {
            match app::RelayAccessToken::new(token.expose().to_string()) {
                Ok(token) => app::RelayProbeCredential::Override(token),
                Err(_) => {
                    return Ok(OperationResult::RelayProbed(RelayProbeOutcome::Other {
                        message: "invalid relay access token".to_string(),
                    }));
                }
            }
        }
    };
    let outcome = match facade.probe_relay_url(&input.url, credential).await {
        Ok(report) => RelayProbeOutcome::Success {
            latency_ms: report.latency_ms,
        },
        Err(app::SettingsFacadeError::RelayProbeInvalidUrl(source)) => {
            RelayProbeOutcome::InvalidUrl {
                message: relay_probe_host_message(source.as_ref()),
            }
        }
        Err(app::SettingsFacadeError::RelayProbeDns(source)) => RelayProbeOutcome::Dns {
            message: relay_probe_host_message(source.as_ref()),
        },
        Err(app::SettingsFacadeError::RelayProbeTls(source)) => RelayProbeOutcome::Tls {
            message: relay_probe_host_message(source.as_ref()),
        },
        Err(app::SettingsFacadeError::RelayProbeHandshake(source)) => {
            RelayProbeOutcome::Handshake {
                message: relay_probe_host_message(source.as_ref()),
            }
        }
        Err(app::SettingsFacadeError::RelayProbeTimeout) => RelayProbeOutcome::Timeout,
        Err(app::SettingsFacadeError::RelayProbeOther(source)) => RelayProbeOutcome::Other {
            message: relay_probe_host_message(source.as_ref()),
        },
        Err(app::SettingsFacadeError::RelayProbeUnavailable) => {
            return Err(EngineError::new(
                PROBE_RELAY_UNAVAILABLE_CODE,
                EngineErrorCategory::Unavailable,
                false,
            ));
        }
        Err(_) => return Err(internal_error(PROBE_RELAY_UNAVAILABLE_CODE)),
    };
    Ok(OperationResult::RelayProbed(outcome))
}

pub(crate) async fn execute_save_relay(
    facade: &AppFacade,
    input: SaveRelayInput,
) -> Result<OperationResult, EngineError> {
    let patch = match map_patch(input.settings) {
        Ok(patch) => patch,
        Err(reason) => {
            return Ok(OperationResult::RelaySaved(SaveRelayOutcome::Rejected {
                reason,
            }));
        }
    };
    let credential = match input.credential {
        RelayCredentialEdit::Keep { url } => app::RelayCredentialEdit::Keep { url },
        RelayCredentialEdit::Set { url, access_token } => {
            let access_token = match app::RelayAccessToken::new(access_token.expose().to_string()) {
                Ok(token) => token,
                Err(_) => {
                    return Ok(OperationResult::RelaySaved(SaveRelayOutcome::Rejected {
                        reason: "invalid relay access token".to_string(),
                    }));
                }
            };
            app::RelayCredentialEdit::Set { url, access_token }
        }
        RelayCredentialEdit::Delete { url } => app::RelayCredentialEdit::Delete { url },
    };

    match facade.save_relay(patch, credential).await {
        Ok(saved) => Ok(OperationResult::RelaySaved(SaveRelayOutcome::Saved {
            settings: Box::new(map_settings(saved.settings)),
            credential_status: RelayCredentialStatus {
                configured: saved.credential_status.configured,
            },
        })),
        Err(app::SettingsFacadeError::Invalid(reason)) => {
            Ok(OperationResult::RelaySaved(SaveRelayOutcome::Rejected {
                reason,
            }))
        }
        Err(error) => Err(map_save_relay_error(error)),
    }
}

fn map_save_relay_error(error: app::SettingsFacadeError) -> EngineError {
    match error {
        error @ app::SettingsFacadeError::RelayCredentialsUnavailable
        | error @ app::SettingsFacadeError::RelayCredentialInvalidUrl
        | error @ app::SettingsFacadeError::RelayCredentialInvalidToken
        | error @ app::SettingsFacadeError::RelayCredentialInvalidTarget
        | error @ app::SettingsFacadeError::RelayCredentialStorage
        | error @ app::SettingsFacadeError::RelayCredentialCorrupt => {
            map_relay_credential_error(error, SAVE_RELAY_FAILED_CODE)
        }
        _ => internal_error(SAVE_RELAY_FAILED_CODE),
    }
}

pub(crate) async fn execute_query_relay_credential(
    facade: &AppFacade,
    input: RelayCredentialInput,
) -> Result<OperationResult, EngineError> {
    let status = facade
        .relay_credential_status(&input.url)
        .map_err(|error| map_relay_credential_error(error, QUERY_RELAY_CREDENTIAL_FAILED_CODE))?;
    Ok(OperationResult::RelayCredentialStatus(
        RelayCredentialStatus {
            configured: status.configured,
        },
    ))
}

fn map_relay_credential_error(error: app::SettingsFacadeError, code: u32) -> EngineError {
    let (category, retryable) = match error {
        app::SettingsFacadeError::RelayCredentialInvalidUrl
        | app::SettingsFacadeError::RelayCredentialInvalidToken
        | app::SettingsFacadeError::RelayCredentialInvalidTarget => {
            (EngineErrorCategory::InvalidInput, false)
        }
        app::SettingsFacadeError::RelayCredentialsUnavailable
        | app::SettingsFacadeError::RelayCredentialStorage => {
            (EngineErrorCategory::Unavailable, true)
        }
        app::SettingsFacadeError::RelayCredentialCorrupt => (EngineErrorCategory::Internal, false),
        _ => (EngineErrorCategory::Internal, false),
    };
    EngineError::new(code, category, retryable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]

    fn relay_settings_persistence_failure_uses_the_relay_save_error_code() {
        let error = map_save_relay_error(app::SettingsFacadeError::Save(anyhow::anyhow!(
            "settings storage unavailable"
        )));

        assert_eq!(error.code(), SAVE_RELAY_FAILED_CODE);

        assert_eq!(error.category(), EngineErrorCategory::Internal);
    }

    #[test]

    fn add_rejects_malformed_relay_access_token_as_invalid_input() {
        let error = map_custom_relay_mutation(CustomRelayMutation::Add {
            url: "https://relay.example".to_string(),

            access_token: Some(crate::SecretString::new("malformed\n token")),
        })
        .unwrap_err();

        assert_eq!(error.code(), SAVE_RELAY_FAILED_CODE);

        assert_eq!(error.category(), EngineErrorCategory::InvalidInput);

        assert!(!error.is_retryable());

        assert!(!format!("{error:?}").contains("malformed"));
    }

    #[test]

    fn edit_rejects_malformed_relay_access_token_as_invalid_input() {
        let error = map_custom_relay_mutation(CustomRelayMutation::Edit {
            previous_url: "https://old-relay.example".to_string(),

            url: "https://relay.example".to_string(),

            access_token: Some(crate::SecretString::new("malformed\n token")),
        })
        .unwrap_err();

        assert_eq!(error.code(), SAVE_RELAY_FAILED_CODE);

        assert_eq!(error.category(), EngineErrorCategory::InvalidInput);

        assert!(!error.is_retryable());

        assert!(!format!("{error:?}").contains("malformed"));
    }
}
