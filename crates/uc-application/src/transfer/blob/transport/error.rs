use std::fmt;
use uc_core::error_class::ErrorClass;
use uc_observability_contract::wire_enum;

wire_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
    pub enum BlobExchangeFailure: &'static str {
        #[error("blob transport capacity exhausted")]
        Busy => "blob_transport_busy",
        #[error("blob transport owner closed")]
        Closed => "blob_transport_closed",
        #[error("blob connectivity failed")]
        Connectivity => "blob_connectivity_failed",
        #[error("blob transport task failed")]
        Task => "blob_transport_task_failed",
        #[error("blob shutdown remains incomplete")]
        ShutdownIncomplete => "blob_shutdown_incomplete",
        #[error("blob exchange cancelled")]
        Cancelled => "blob_exchange_cancelled",
        #[error("blob request deadline expired")]
        RequestExpired => "blob_request_expired",
        #[error("blob request is not supported")]
        InvalidRequest => "blob_request_invalid",
        #[error("blob source is unavailable")]
        SourceUnavailable => "blob_source_unavailable",
        #[error("blob exchange network failed")]
        Network => "blob_exchange_network_failed",
        #[error("blob exchange temporary network failure")]
        TransientNetwork => "blob_exchange_temporary_network_failed",
        #[error("blob content verification failed")]
        Integrity => "blob_integrity_failed",
        #[error("blob local store failed")]
        Store => "blob_exchange_store_failed",
        #[error("blob export ended with an incomplete response")]
        IncompleteExport => "blob_export_incomplete",
        #[error("blob local settlement failed")]
        Settlement => "blob_exchange_settlement_failed",
    }
}

/// 与生命周期报告一致，标准 source 指向主因，额外停止／结算失败原样保存在报告中。
#[derive(thiserror::Error)]
#[error("blob exchange failed: {kind:?}")]
pub struct BlobExchangeError {
    pub kind: BlobExchangeFailure,
    #[source]
    pub source: anyhow::Error,
    pub additional: Vec<BlobExchangeError>,
}
impl fmt::Debug for BlobExchangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BlobExchangeError")
            .field("kind", &self.kind)
            .field("additional_count", &self.additional.len())
            .finish()
    }
}
impl ErrorClass for BlobExchangeError {
    fn class(&self) -> &'static str {
        self.kind.wire()
    }
}
impl BlobExchangeError {
    pub fn new(kind: BlobExchangeFailure, source: impl Into<anyhow::Error>) -> Self {
        Self {
            kind,
            source: source.into(),
            additional: Vec::new(),
        }
    }

    pub fn refused(reason: BlobExchangeFailure) -> Self {
        Self {
            kind: reason,
            source: reason.into(),
            additional: Vec::new(),
        }
    }

    pub fn with_settlement(self, settlement: BlobExchangeError) -> Self {
        Self {
            kind: BlobExchangeFailure::Settlement,
            source: self.into(),
            additional: vec![settlement],
        }
    }
}
