use uc_core::membership::{
    AdmissionEncryptedPasswordEquivalent, AdmissionJoinerPrivateState, AdmissionJoinerStartContext,
    AdmissionShortInvitationCode, AdmissionSourceSnapshot, JoinId, JoinerAdmission,
    JoinerAdmissionTransition, SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionRoute,
};

pub enum PreparedJoinerInvitation {
    Full,
    Short {
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        start_context: AdmissionJoinerStartContext,
        short_code: AdmissionShortInvitationCode,
    },
}

impl PreparedJoinerInvitation {
    pub fn short(
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        start_context: AdmissionJoinerStartContext,
        short_code: AdmissionShortInvitationCode,
    ) -> Self {
        Self::Short {
            admission_id,
            join_id,
            start_context,
            short_code,
        }
    }
}

pub struct JoinerStartMaterial {
    admission_id: SpaceAdmissionId,
    join_id: JoinId,
    route: SpaceAdmissionRoute,
    join_request: SpaceAdmissionEnvelopeV1,
    private_state: AdmissionJoinerPrivateState,
    encrypted_password_equivalent: AdmissionEncryptedPasswordEquivalent,
}

impl JoinerStartMaterial {
    pub fn new(
        admission_id: SpaceAdmissionId,
        join_id: JoinId,
        route: SpaceAdmissionRoute,
        join_request: SpaceAdmissionEnvelopeV1,
        private_state: AdmissionJoinerPrivateState,
        encrypted_password_equivalent: AdmissionEncryptedPasswordEquivalent,
    ) -> Self {
        Self {
            admission_id,
            join_id,
            route,
            join_request,
            private_state,
            encrypted_password_equivalent,
        }
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        SpaceAdmissionId,
        JoinId,
        SpaceAdmissionRoute,
        SpaceAdmissionEnvelopeV1,
        AdmissionJoinerPrivateState,
        AdmissionEncryptedPasswordEquivalent,
    ) {
        (
            self.admission_id,
            self.join_id,
            self.route,
            self.join_request,
            self.private_state,
            self.encrypted_password_equivalent,
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SpaceAdmissionCommitToken([u8; 32]);

impl SpaceAdmissionCommitToken {
    pub fn from_bytes(bytes: [u8; 32]) -> Option<Self> {
        if bytes == [0; 32] {
            None
        } else {
            Some(Self(bytes))
        }
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SpaceAdmissionCommitToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SpaceAdmissionCommitToken([REDACTED])")
    }
}

/// 一次公开 JoinSpace 开始前读取到的完整业务视图。
///
/// 这些事实必须来自同一次读取，`SpaceAdmissionProtocol` 只能整体消费它们来判断是创建新加入、
/// 取代尚可取代的旧加入，还是拒绝开始。它不是可独立修改的状态机，也不暴露成员账本内部字段。
pub struct LoadedJoinerStartState {
    /// 本次新加入必须使用的本机单调序号。
    next_local_join_ordinal: u64,
    /// 开始加入时仍需保留的来源 Space 事实，供后续切换和恢复使用。
    source_snapshot: AdmissionSourceSnapshot,
    /// 当前尚未结束的本机加入；用于判断能否安全取代，而不是另开一条并行流程。
    current_join: Option<JoinerAdmission>,
    /// 本次加入完成后是否需要切换当前 Space 会话。
    requires_session_transition: bool,
    /// 绑定本次读取结果的不透明凭证；提交完整变化时必须原样交回。
    commit_token: SpaceAdmissionCommitToken,
}

impl LoadedJoinerStartState {
    /// 建立一份来自同一次读取的开始加入视图。
    pub fn new(
        next_local_join_ordinal: u64,
        source_snapshot: AdmissionSourceSnapshot,
        current_join: Option<JoinerAdmission>,
        requires_session_transition: bool,
        commit_token: SpaceAdmissionCommitToken,
    ) -> Self {
        Self {
            next_local_join_ordinal,
            source_snapshot,
            current_join,
            requires_session_transition,
            commit_token,
        }
    }

    /// 一次性取出开始加入所需事实，避免旧视图被重复用于后续提交。
    pub fn into_parts(
        self,
    ) -> (
        u64,
        AdmissionSourceSnapshot,
        Option<JoinerAdmission>,
        bool,
        SpaceAdmissionCommitToken,
    ) {
        (
            self.next_local_join_ordinal,
            self.source_snapshot,
            self.current_join,
            self.requires_session_transition,
            self.commit_token,
        )
    }
}

/// 一次开始新加入必须共同保存的完整变化。
pub struct JoinerStartMutation {
    created: JoinerAdmissionTransition,
    superseded: Option<JoinerAdmissionTransition>,
}

impl JoinerStartMutation {
    pub fn new(
        created: JoinerAdmissionTransition,
        superseded: Option<JoinerAdmissionTransition>,
    ) -> Self {
        Self {
            created,
            superseded,
        }
    }

    pub fn into_parts(self) -> (JoinerAdmissionTransition, Option<JoinerAdmissionTransition>) {
        (self.created, self.superseded)
    }
}
