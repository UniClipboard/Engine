//! Stable observability interfaces available to cross-platform host adapters.

pub use uc_observability_contract::analytics::{
    AdoptOutcome, AnalyticsIdentityError, AnalyticsIdentityPort, AnalyticsPort, Event,
    GroupIdentifyPayload, IdentifyPayload, ReleaseOutcome,
};
