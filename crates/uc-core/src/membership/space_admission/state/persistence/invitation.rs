use super::*;

impl From<&SpaceAdmissionJoinerResolvingInvitation> for PersistedJoinerResolvingInvitationV1 {
    fn from(state: &SpaceAdmissionJoinerResolvingInvitation) -> Self {
        let resolution = match &state.resolution {
            SpaceAdmissionInvitationResolutionState::Ready { short_code } => {
                PersistedInvitationResolutionV1::Ready {
                    short_code: short_code.as_bytes().to_vec(),
                }
            }
            SpaceAdmissionInvitationResolutionState::Started => {
                PersistedInvitationResolutionV1::Started
            }
        };
        Self {
            join_id: *state.join_id.as_bytes(),
            local_join_ordinal: state.local_join_ordinal,
            source_snapshot: state.source_snapshot.as_bytes().to_vec(),
            start_context: state.start_context.as_bytes().to_vec(),
            resolution,
        }
    }
}

impl PersistedJoinerResolvingInvitationV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionJoinerResolvingInvitation, SpaceAdmissionPersistenceError> {
        let resolution = match self.resolution {
            PersistedInvitationResolutionV1::Ready { short_code } => {
                SpaceAdmissionInvitationResolutionState::Ready {
                    short_code: AdmissionShortInvitationCode::from_bytes(short_code)
                        .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
                }
            }
            PersistedInvitationResolutionV1::Started => {
                SpaceAdmissionInvitationResolutionState::Started
            }
        };
        Ok(SpaceAdmissionJoinerResolvingInvitation {
            join_id: JoinId::from_bytes(self.join_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            local_join_ordinal: self.local_join_ordinal,
            source_snapshot: AdmissionSourceSnapshot::from_bytes(self.source_snapshot)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            start_context: AdmissionJoinerStartContext::from_bytes(self.start_context)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            resolution,
        })
    }
}

impl From<&SpaceAdmissionJoinerResolvedInvitation> for PersistedJoinerResolvedInvitationV1 {
    fn from(state: &SpaceAdmissionJoinerResolvedInvitation) -> Self {
        Self {
            join_id: *state.join_id.as_bytes(),
            local_join_ordinal: state.local_join_ordinal,
            source_snapshot: state.source_snapshot.as_bytes().to_vec(),
            start_context: state.start_context.as_bytes().to_vec(),
            full_invitation: state.full_invitation.as_str().to_owned(),
        }
    }
}

impl PersistedJoinerResolvedInvitationV1 {
    pub(super) fn into_domain(
        self,
    ) -> Result<SpaceAdmissionJoinerResolvedInvitation, SpaceAdmissionPersistenceError> {
        Ok(SpaceAdmissionJoinerResolvedInvitation {
            join_id: JoinId::from_bytes(self.join_id)
                .ok_or(SpaceAdmissionPersistenceError::InvalidState)?,
            local_join_ordinal: self.local_join_ordinal,
            source_snapshot: AdmissionSourceSnapshot::from_bytes(self.source_snapshot)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            start_context: AdmissionJoinerStartContext::from_bytes(self.start_context)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
            full_invitation: FullInvitation::new(self.full_invitation)
                .map_err(|_| SpaceAdmissionPersistenceError::InvalidState)?,
        })
    }
}
