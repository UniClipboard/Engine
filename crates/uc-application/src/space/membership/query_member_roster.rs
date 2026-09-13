//! 当前 Space 成员名单查询。

use std::sync::Arc;

use thiserror::Error;
use tracing::instrument;

use uc_core::membership::{MemberRepositoryPort, MembershipError};
use uc_core::ports::{
    LocalIdentityError, LocalIdentityPort, PeerReachabilityPort, ReachabilityState,
};
use uc_core::DeviceId;

use crate::deps::{CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort};

/// 当前 Space 成员名单中的一项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    /// 跨设备重命名保持不变的成员标识。
    pub device_id: DeviceId,
    /// 面向用户展示的设备名称。
    pub device_name: String,
    /// 是否为本机成员。
    pub is_local: bool,
    /// 当前已知在线状态；读取该值不会发起网络连接。
    pub state: ReachabilityState,
}

#[derive(Debug, Error)]
pub(crate) enum QueryMemberRosterError {
    #[error("成员资料读取失败")]
    MemberRepository {
        #[source]
        source: MembershipError,
    },
    #[error("当前成员范围不可用")]
    MemberScope {
        #[source]
        source: CurrentSpaceMemberScopeError,
    },
    #[error("本机身份读取失败")]
    LocalIdentity {
        #[source]
        source: LocalIdentityError,
    },
}

pub(crate) struct QueryMemberRosterUseCase {
    member_repo: Arc<dyn MemberRepositoryPort>,
    local_identity: Arc<dyn LocalIdentityPort>,
    peer_reachability: Arc<dyn PeerReachabilityPort>,
    peer_scope: Arc<dyn CurrentSpaceMemberScopePort>,
}

impl QueryMemberRosterUseCase {
    pub(crate) fn new(
        member_repo: Arc<dyn MemberRepositoryPort>,
        local_identity: Arc<dyn LocalIdentityPort>,
        peer_reachability: Arc<dyn PeerReachabilityPort>,
        peer_scope: Arc<dyn CurrentSpaceMemberScopePort>,
    ) -> Self {
        Self {
            member_repo,
            local_identity,
            peer_reachability,
            peer_scope,
        }
    }

    #[instrument(skip_all)]
    pub(crate) async fn execute(&self) -> Result<Vec<RosterEntry>, QueryMemberRosterError> {
        let members = self
            .member_repo
            .list()
            .await
            .map_err(|source| QueryMemberRosterError::MemberRepository { source })?;
        let scope = self
            .peer_scope
            .snapshot()
            .await
            .map_err(|source| QueryMemberRosterError::MemberScope { source })?;
        let local_fingerprint = self
            .local_identity
            .get_current_fingerprint()
            .await
            .map_err(|source| QueryMemberRosterError::LocalIdentity { source })?;

        let mut entries = Vec::with_capacity(members.len());
        for member in members {
            let is_local = local_fingerprint
                .as_ref()
                .is_some_and(|fingerprint| fingerprint == &member.identity_fingerprint);
            let is_current_peer = scope.usable_peer_device_ids.contains(&member.device_id)
                || scope
                    .paused_peer_devices
                    .iter()
                    .any(|peer| peer.device_id == member.device_id);
            if !is_local && !is_current_peer {
                continue;
            }
            entries.push(RosterEntry {
                state: self
                    .peer_reachability
                    .current_state(&member.device_id)
                    .await,
                device_id: member.device_id,
                device_name: member.device_name,
                is_local,
            });
        }
        Ok(entries)
    }
}
