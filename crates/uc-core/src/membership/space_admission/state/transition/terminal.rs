use super::*;

impl SpaceAdmissionAggregate {
    pub fn require_recovery(
        mut self,
        category: AdmissionRecoveryCategory,
    ) -> Result<AdmissionTransition, SpaceAdmissionAggregateError> {
        if matches!(self.state, SpaceAdmissionRecordState::Terminal(_)) {
            return Err(SpaceAdmissionAggregateError::InvalidTransition);
        }
        let record_version = self
            .record_version
            .checked_add(1)
            .ok_or(SpaceAdmissionAggregateError::RecordVersionOverflow)?;
        self.record_version = record_version;
        self.state =
            SpaceAdmissionRecordState::Terminal(SpaceAdmissionTerminalState::RecoveryRequired(
                SpaceAdmissionRecoveryRequiredTerminal { category },
            ));
        Ok(AdmissionTransition::new(self, &[]))
    }
}
