mod use_case;

pub(crate) use use_case::RecoverPendingAdmissionsUseCase;

#[cfg(test)]
pub(crate) use use_case::{
    record_protocol_message_delivered, recover_outbox_deliveries, AdmissionRecoveryReportV1,
};
