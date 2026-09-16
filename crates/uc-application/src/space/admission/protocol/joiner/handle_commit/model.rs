use uc_core::membership::PendingAdmissionExchange;

pub struct PreparedJoinerAppliedMaterial {
    pending_exchange: PendingAdmissionExchange,
}

impl PreparedJoinerAppliedMaterial {
    pub fn new(pending_exchange: PendingAdmissionExchange) -> Self {
        Self { pending_exchange }
    }

    pub fn into_pending_exchange(self) -> PendingAdmissionExchange {
        self.pending_exchange
    }
}
