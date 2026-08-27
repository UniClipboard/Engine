mod current_scope;
mod effect_executor;
mod error;
mod initializer;
mod join_record;
mod model;
mod repository;
mod restricted_delivery;

pub use current_scope::{
    CurrentSpaceMemberScope, CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort,
    PausedSpaceMember, SpaceMemberPauseReason,
};
pub(crate) use effect_executor::RePairingAwareMembershipActivation;
pub(crate) use effect_executor::RecoverMembershipEffectsUseCase;
pub use effect_executor::{
    ActivateMembershipEffectPort, ApplyMembershipMemberFactsPort, ApplyMembershipSecurityPort,
    MembershipEffectExecutionError,
};
pub use error::MembershipLedgerError;
pub(crate) use initializer::InitializeSpaceMembershipUseCase;
pub use model::{
    InboundMembershipTransfer, LoadedMembershipLedger, MembershipEffectKind, MembershipEffectPhase,
    MembershipLedgerMutation, PeerReconciliationRecord, PendingMembershipEffect,
    RestrictedMembershipDelivery,
};
pub use repository::{CommitMembershipLedgerPort, LoadMembershipLedgerPort};
pub(crate) use repository::{MembershipLedger, VerifiedMembershipLedger};
pub(crate) use restricted_delivery::DeliverRestrictedMembershipUseCase;
pub use restricted_delivery::{
    RestrictedMembershipDeliveryError, RestrictedMembershipDeliveryPort,
};

#[cfg(test)]
mod tests;
