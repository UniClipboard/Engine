mod credentials;
mod digest;
mod full_invitation;
mod joiner;
mod recovery;
mod recovery_material;
mod repository;
mod security;
mod sponsor;

#[cfg(test)]
pub(crate) use credentials::prepare_registration;
pub(crate) use credentials::{
    install_prepared_registration_for_control_generation,
    rebind_registration_to_control_generation, upgrade_registration_to_control_generation,
    verify_prepared_registration_for_control_generation,
};
pub use credentials::{SpaceAdmissionCredentialStoreError, SqliteSpaceAdmissionCredentials};
#[cfg(test)]
pub(crate) use full_invitation::decode_full_invitation;
pub(crate) use full_invitation::{decode_invitation_entry, encode_full_invitation};
pub use joiner::{
    DefaultJoinerActivationExecutor, DefaultJoinerActivationPreparation,
    DefaultJoinerAppliedPreparation, DefaultJoinerCancellationPreparation,
    DefaultJoinerCandidatePreparation, DefaultJoinerInvitationPreparation,
    DefaultJoinerStartMaterial,
};
pub use repository::SqliteSpaceAdmissionState;
pub use security::AdmissionSecurityTransitionAdapter;
pub use sponsor::{
    DefaultSponsorAdmissionActivation, DefaultSponsorCandidatePreparation,
    DefaultSponsorCommitPreparation, DefaultSponsorCompletePreparation,
    DefaultSponsorSettledPreparation,
};
