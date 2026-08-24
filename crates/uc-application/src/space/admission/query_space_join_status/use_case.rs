use std::sync::Arc;

use super::error::QuerySpaceJoinStatusError;

use crate::deps::AdmissionAttemptRepositoryPort;
use crate::space::admission::{CurrentJoinStatus, JoinedSpace};
use uc_core::membership::{
    AdmissionIdentityBindingV1, AdmissionSpaceTransitionResultV2, AdmissionTerminalResultV1,
};

pub(crate) struct QuerySpaceJoinStatusUseCase {
    repo: Arc<dyn AdmissionAttemptRepositoryPort>,
}

impl QuerySpaceJoinStatusUseCase {
    pub(crate) fn new(repo: Arc<dyn AdmissionAttemptRepositoryPort>) -> Self {
        Self { repo }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<Option<CurrentJoinStatus>, QuerySpaceJoinStatusError> {
        let Some(projection) = self
            .repo
            .project_current_local_join()
            .await
            .map_err(QuerySpaceJoinStatusError::repository)?
        else {
            return Ok(None);
        };

        if projection.terminal_result.is_none() {
            let attempt = self
                .repo
                .load(projection.attempt_id)
                .await
                .map_err(QuerySpaceJoinStatusError::repository)?
                .ok_or_else(|| {
                    QuerySpaceJoinStatusError::invalid_state(
                        "current local join attempt is missing",
                    )
                })?;

            let binding = attempt
                .identity_binding
                .as_deref()
                .map(AdmissionIdentityBindingV1::decode)
                .transpose()
                .map_err(QuerySpaceJoinStatusError::invalid_state)?;

            return Ok(Some(CurrentJoinStatus::Pending {
                join_id: projection.join_id,
                target_space_id: attempt.lineage_id,
                sponsor_device_id: binding
                    .as_ref()
                    .map(|binding| binding.sponsor_device_id.clone()),
                sponsor_identity_fingerprint: binding
                    .map(|binding| binding.sponsor_identity_fingerprint),
                cancel_requested: attempt.cancel_request.is_some(),
            }));
        }

        match projection.terminal_result {
            Some(AdmissionTerminalResultV1::Rejected) => {
                let reason = projection.rejection_reason.ok_or_else(|| {
                    QuerySpaceJoinStatusError::invalid_state(
                        "rejected local join reason is missing",
                    )
                })?;

                Ok(Some(CurrentJoinStatus::Rejected {
                    join_id: projection.join_id,
                    reason,
                }))
            }

            Some(AdmissionTerminalResultV1::Active) => {
                let terminal = self
                    .repo
                    .load_terminal(projection.attempt_id)
                    .await
                    .map_err(QuerySpaceJoinStatusError::repository)?
                    .ok_or_else(|| {
                        QuerySpaceJoinStatusError::invalid_state(
                            "active local join terminal is missing",
                        )
                    })?;

                let binding = AdmissionIdentityBindingV1::decode(
                    terminal.identity_binding.as_deref().ok_or_else(|| {
                        QuerySpaceJoinStatusError::invalid_state(
                            "active local join identity is missing",
                        )
                    })?,
                )
                .map_err(QuerySpaceJoinStatusError::invalid_state)?;

                let transition_result = terminal
                    .space_transition_result
                    .as_deref()
                    .and_then(AdmissionSpaceTransitionResultV2::decode);

                let (migrated_records, preserved_unreadable_records) = match transition_result {
                    Some(AdmissionSpaceTransitionResultV2::CrossSpace(result)) => (
                        Some(result.migrated_records),
                        Some(result.preserved_unreadable_records),
                    ),

                    Some(AdmissionSpaceTransitionResultV2::SameSpace { .. }) => (Some(0), Some(0)),

                    Some(AdmissionSpaceTransitionResultV2::Fresh { .. }) | None => (None, None),
                };

                Ok(Some(CurrentJoinStatus::Active {
                    join_id: projection.join_id,
                    joined_space: JoinedSpace {
                        sponsor_device_id: binding.sponsor_device_id,
                        sponsor_identity_fingerprint: binding.sponsor_identity_fingerprint,
                        space_id: binding.lineage_id,
                        self_device_id: binding.joiner_device_id,
                        self_identity_fingerprint: binding.joiner_identity_fingerprint,
                        migrated_records,
                        preserved_unreadable_records,
                    },
                }))
            }

            Some(AdmissionTerminalResultV1::Completed) => {
                Err(QuerySpaceJoinStatusError::invalid_state(
                    "local join has a sponsor-only completion result",
                ))
            }

            Some(AdmissionTerminalResultV1::SupersededByNewJoin) => Err(
                QuerySpaceJoinStatusError::invalid_state("superseded join was selected as current"),
            ),

            None => Err(QuerySpaceJoinStatusError::invalid_state(
                "local join projection changed unexpectedly",
            )),
        }
    }
}
