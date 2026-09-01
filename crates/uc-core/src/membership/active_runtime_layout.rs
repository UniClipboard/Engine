use crate::ids::SpaceId;

/// 活动运行期同时引用的 profile 数据与 Space 控制世代。
///
/// profile 数据世代跨 Space 保留；Space 控制世代只属于当前 Space。
#[derive(Clone, PartialEq, Eq)]
pub struct ActiveRuntimeLayout {
    space_id: SpaceId,
    profile_data_generation: [u8; 16],
    space_control_generation: [u8; 16],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ActiveRuntimeLayoutError {
    #[error("active runtime space id is empty")]
    EmptySpaceId,
    #[error("active runtime generation uses a reserved value")]
    ReservedGeneration,
    #[error("profile data and space control generations are aliased")]
    AliasedGenerations,
}

impl ActiveRuntimeLayout {
    pub fn new(
        space_id: SpaceId,
        profile_data_generation: [u8; 16],
        space_control_generation: [u8; 16],
    ) -> Result<Self, ActiveRuntimeLayoutError> {
        if space_id.as_ref().is_empty() {
            return Err(ActiveRuntimeLayoutError::EmptySpaceId);
        }
        if profile_data_generation == [0; 16] || space_control_generation == [0; 16] {
            return Err(ActiveRuntimeLayoutError::ReservedGeneration);
        }
        if profile_data_generation == space_control_generation {
            return Err(ActiveRuntimeLayoutError::AliasedGenerations);
        }
        Ok(Self {
            space_id,
            profile_data_generation,
            space_control_generation,
        })
    }

    pub const fn space_id(&self) -> &SpaceId {
        &self.space_id
    }

    pub const fn profile_data_generation(&self) -> &[u8; 16] {
        &self.profile_data_generation
    }

    pub const fn space_control_generation(&self) -> &[u8; 16] {
        &self.space_control_generation
    }
}

impl std::fmt::Debug for ActiveRuntimeLayout {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActiveRuntimeLayout")
            .field("identifiers", &"[REDACTED]")
            .finish()
    }
}
