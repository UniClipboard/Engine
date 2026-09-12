//! 有限的本地采集来源；不接受宿主自由命名的来源。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum LocalDiagnosticSource {
    Runtime,
    Connections,
    AddressStorage,
    DnsDiscovery,
    MdnsDiscovery,
    PkarrDiscovery,
    ConnectionPaths,
    RelayRecovery,
    MembershipUpdates,
    Sessions,
    HostApplication,
    HostShareExtension,
    HostKeyboardExtension,
    HostBackgroundService,
}

impl LocalDiagnosticSource {
    pub const ALL: [Self; 14] = [
        Self::Runtime,
        Self::Connections,
        Self::AddressStorage,
        Self::DnsDiscovery,
        Self::MdnsDiscovery,
        Self::PkarrDiscovery,
        Self::ConnectionPaths,
        Self::RelayRecovery,
        Self::MembershipUpdates,
        Self::Sessions,
        Self::HostApplication,
        Self::HostShareExtension,
        Self::HostKeyboardExtension,
        Self::HostBackgroundService,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCapability {
    Supported,
    Partial,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceCollection {
    Enabled,
    Disabled,
    Unavailable,
    NotRegistered,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceEvent {
    pub source: LocalDiagnosticSource,
    pub capability: SourceCapability,
    pub collection: SourceCollection,
}

impl super::NetworkRecorder {
    pub fn register_source(
        &self,
        source: LocalDiagnosticSource,
        capability: SourceCapability,
        collection: SourceCollection,
    ) {
        let context = super::ObservationContext::capture();
        tracing::dispatcher::with_default(&self.dispatcher, || {
            super::emit_local(
                super::record::LocalEvent::Source {
                    record: SourceEvent {
                        source,
                        capability,
                        collection,
                    },
                },
                &context,
            )
        });
    }
}
