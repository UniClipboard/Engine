mod adapters;
mod admission;
mod encryption_passphrase_change;
mod membership_branch_transition;
pub(crate) mod membership_record;

pub use adapters::{
    CurrentSpaceResolver, DeviceTrustObservationsAdapter, EncryptedRePairingStateStore,
    FileSpaceRebuildProgress, GatedMembershipHistoryExchange, GatedSpaceAdmissionTransport,
    MembershipActivationAdapter, MembershipMemberFactsAdapter, MembershipNetworkGate,
};
#[cfg(test)]
pub(crate) use admission::decode_full_invitation;
pub(crate) use admission::prepare_registration;
#[cfg(feature = "test-util")]
pub use admission::AdmissionRepositoryBenchmark;
pub(crate) use admission::{decode_invitation_entry, encode_full_invitation};
pub(crate) use admission::{
    install_prepared_registration_for_control_generation,
    rebind_registration_to_control_generation, upgrade_registration_to_control_generation,
    verify_prepared_registration_for_control_generation,
};
pub use admission::{
    AdmissionSecurityTransitionAdapter, DefaultJoinerActivationExecutor,
    DefaultJoinerActivationPreparation, DefaultJoinerAppliedPreparation,
    DefaultJoinerCancellationPreparation, DefaultJoinerCandidatePreparation,
    DefaultJoinerInvitationPreparation, DefaultJoinerStartMaterial,
    DefaultSponsorAdmissionActivation, DefaultSponsorCandidatePreparation,
    DefaultSponsorCommitPreparation, DefaultSponsorCompletePreparation,
    DefaultSponsorSettledPreparation, SpaceAdmissionCredentialStoreError,
    SqliteSpaceAdmissionCredentials, SqliteSpaceAdmissionState,
};
pub use encryption_passphrase_change::EncryptionPassphraseChange;
pub use membership_branch_transition::DefaultMembershipBranchTransitionPreparation;
pub use membership_record::SqliteMembershipRecordStore;
pub use uc_infra_crypto::history_signature::OpenMlsHistoricalSignatureVerifier;
pub(crate) use uc_infra_security::group_update_failure_detail;
pub use uc_infra_security::{
    DefaultMembershipSecurityUpdateAdapter, InMemorySession, KeyMaterialStore,
    MigrationSpaceAccessAdapter, RuntimeSpaceAccessAdapter, SpaceSessionRebindAdapter,
};
