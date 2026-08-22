#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RePairingStateError {
    #[error("re-pairing state is unavailable")]
    Unavailable,

    #[error("re-pairing state is inconsistent")]
    Inconsistent,
}
