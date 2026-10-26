//! 在发布新成员事实前同步退休旧资格；watch 合并提示不能跳过这一步。
use super::view::SpaceMembershipView;
use super::{MembershipView, Published};
use std::collections::BTreeMap;
use uc_core::ids::DeviceId;
use uc_core::membership::{LedgerMemberStatus, MemberInstanceId, MembershipEventId};

pub(crate) enum MemberScopeRetirement {
    All,
    Peers(Vec<DeviceId>),
}
/// 只清理会话内派生关联，不改写成员、文件或网络终态；实现不得重入成员 Owner。
pub(crate) trait MemberScopeRetirementPort: Send + Sync {
    fn retire_membership(&self, retirement: MemberScopeRetirement);
}

pub(super) fn between(
    previous: Option<&Published>,
    generation: u64,
    next: &MembershipView,
) -> Option<MemberScopeRetirement> {
    let previous = previous?;
    if previous.generation != generation {
        return Some(MemberScopeRetirement::All);
    }
    let (Some(old), Some(new)) = (previous.view.space(), next.space()) else {
        return Some(MemberScopeRetirement::All);
    };
    if old.history().lineage_id() != new.history().lineage_id()
        || old.local_member() != new.local_member()
        || old.history().admission_event_id_for(old.local_member())
            != new.history().admission_event_id_for(new.local_member())
        || new.ledger().local_status() != LedgerMemberStatus::Active
    {
        return Some(MemberScopeRetirement::All);
    }
    let current = identities(new);
    let peers = identities(old)
        .into_iter()
        .filter_map(|(device, identity)| {
            (current.get(&device) != Some(&identity)).then_some(device)
        })
        .collect::<Vec<_>>();
    (!peers.is_empty()).then_some(MemberScopeRetirement::Peers(peers))
}

fn identities(
    space: &SpaceMembershipView,
) -> BTreeMap<DeviceId, (MemberInstanceId, Option<MembershipEventId>)> {
    let history = space.history();
    let candidates = space
        .ledger()
        .peers()
        .map(|(device, _)| *device)
        .chain(std::iter::once(*space.local_device_id()))
        .collect::<Vec<_>>();
    history
        .active_members()
        .into_iter()
        .filter_map(|member| {
            let device = history.device_for_member(&member, &candidates)?;
            (device != *space.local_device_id())
                .then_some((device, (member, history.admission_event_id_for(member))))
        })
        .collect()
}
