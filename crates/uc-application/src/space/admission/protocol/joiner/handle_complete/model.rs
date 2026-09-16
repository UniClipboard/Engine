use uc_core::membership::AdmissionSpaceTransition;

pub struct PreparedJoinerActivation {
    transition: AdmissionSpaceTransition,
}

impl PreparedJoinerActivation {
    pub fn new(transition: AdmissionSpaceTransition) -> Self {
        Self { transition }
    }

    pub(crate) fn into_transition(self) -> AdmissionSpaceTransition {
        self.transition
    }
}
