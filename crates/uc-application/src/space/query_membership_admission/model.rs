use uc_core::membership::MembershipAdmissionDecision;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MembershipAdmissionSnapshot {
    pub current_generation: u64,
    pub decision: MembershipAdmissionDecision,
}
