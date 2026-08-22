#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfileKeyAccessProbe {
    Available,
    PermissionDenied,
    TemporarilyUnavailable,
    Missing,
}
