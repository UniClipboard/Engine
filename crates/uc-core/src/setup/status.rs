use crate::ids::SpaceId;

/// Setup status persisted across app restarts.
///
/// 设置流程持久化状态。
///
/// `space_id` is populated by A1 `InitializeSpaceUseCase` with the id
/// minted during first-time space creation and persisted forever after
/// — it's the canonical identifier every downstream consumer (A2
/// unlock, sponsor handshake, joiner's mirrored record) must agree on.
/// Older installs that pre-date this field appear as `None`; callers must use
/// the stable legacy Space identity so encrypted state remains readable across
/// restarts.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SetupStatus {
    pub has_completed: bool,
    #[serde(default)]
    pub space_id: Option<SpaceId>,
    #[serde(default)]
    pub re_pairing_required: bool,
}

impl Default for SetupStatus {
    fn default() -> Self {
        Self {
            has_completed: false,
            space_id: None,
            re_pairing_required: false,
        }
    }
}
