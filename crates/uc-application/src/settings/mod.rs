//! Settings-scoped application workflows.
//!
//! The complete result — settings read/validate/save, upgrade detection and
//! acknowledgement, config migration, storage statistics and relay
//! diagnostics — stays inside this directory.

pub(crate) mod assembly;
pub(crate) mod config_migration;
pub(crate) mod diagnostics;
mod facade;
mod models;
pub(crate) mod relay_configuration;
pub(crate) mod relay_credentials;
pub(crate) mod relay_diagnostic;
pub(crate) mod storage;
pub(crate) mod upgrade;

pub use assembly::{PreparedNetworkSettings, SettingsAssembly, SettingsAssemblyParts};
pub use facade::{
    RelayCredentialStatusView, RelayProbeReportView, RelaySaveView, SettingsFacade,
    SettingsFacadeError,
};
pub use models::{
    CongestionControllerView, ContentTypesPatch, ContentTypesView, FileSyncSettingsPatch,
    FileSyncSettingsView, GeneralSettingsPatch, GeneralSettingsView, NetworkSettingsPatch,
    NetworkSettingsView, PairingSettingsPatch, PairingSettingsView, RetentionPolicyPatch,
    RetentionPolicyView, RetentionRulePatchValue, RetentionRuleView, RuleEvaluationView,
    SecuritySettingsPatch, SecuritySettingsView, SettingsPatch, SettingsView, SyncFrequencyView,
    SyncSettingsPatch, SyncSettingsView,
};
pub use relay_configuration::{
    AppliedRelayRouting, RelayConfiguration, RelayConfigurationEntry, RelayConfigurationMutation,
    RelayConfigurationRejection, RelayEntrySource, RelayOverview, RelayOverviewEntry,
};
pub use relay_credentials::{
    RelayAccessToken, RelayCredentialEdit, RelayCredentials, RelayCredentialsError,
    RelayProbeCredential,
};
pub use relay_diagnostic::{RelayDiagnosticPort, RelayProbeError, RelayProbeReport};
