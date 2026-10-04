mod credentials;
mod digest;
mod display;
mod failure_log;
mod joiner;
mod recovery;
mod recovery_material;
mod repository;
mod security;
mod sponsor;

pub(crate) use credentials::prepare_registration;
pub(crate) use credentials::{
    install_prepared_registration_for_control_generation,
    rebind_registration_to_control_generation, upgrade_registration_to_control_generation,
    verify_prepared_registration_for_control_generation,
};
pub use credentials::{SpaceAdmissionCredentialStoreError, SqliteSpaceAdmissionCredentials};
pub use joiner::{
    DefaultJoinerActivationExecutor, DefaultJoinerActivationPreparation,
    DefaultJoinerAppliedPreparation, DefaultJoinerCancellationPreparation,
    DefaultJoinerCandidatePreparation, DefaultJoinerInvitationPreparation,
    DefaultJoinerStartMaterial,
};
#[cfg(feature = "test-util")]
pub use repository::AdmissionRepositoryBenchmark;
pub use repository::SqliteSpaceAdmissionState;
pub use security::AdmissionSecurityTransitionAdapter;
pub use sponsor::{
    DefaultSponsorAdmissionActivation, DefaultSponsorCandidatePreparation,
    DefaultSponsorCommitPreparation, DefaultSponsorCompletePreparation,
    DefaultSponsorSettledPreparation,
};
