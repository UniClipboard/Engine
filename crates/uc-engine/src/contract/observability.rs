//! Stable observability interfaces available to cross-platform host adapters.

pub use uc_observability_contract::analytics::{
    AdoptOutcome, AnalyticsEventContext, AnalyticsIdentityError, AnalyticsIdentityPort,
    AnalyticsPort, DeviceType, Event, GroupIdentifyPayload, IdentifyPayload, Os, ReleaseOutcome,
};
