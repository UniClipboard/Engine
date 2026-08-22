#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpaceAccessState {
    pub initialized: bool,
    pub session_ready: bool,
}
