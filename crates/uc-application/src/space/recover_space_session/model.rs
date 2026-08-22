#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoverSpaceSessionResult {
    pub unlocked: bool,
    pub resumed: bool,
}
