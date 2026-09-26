use crate::ids::DeviceId;
use crate::membership::{MembershipDecisionV2, MembershipEventV2};

use super::{
    LedgerMemberStatus, LedgerTransitionError, MembershipLedger, PeerLink, UnfinishedMemberEffect,
};

/// 由成员状态计算出的一项持久待办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LedgerWork {
    /// 执行成员效果的当前阶段。
    AdvanceEffect(UnfinishedMemberEffect),
    /// 向已被本机移除的设备投递移除通知。
    DeliverRemovalNotice {
        peer: DeviceId,
        notice: MembershipEventV2,
    },
    /// 离开窗口到期，结束对该设备的通知责任。
    EndDeparture { peer: DeviceId },
    /// 向移除发起方投递本机决定。
    DeliverDecision {
        peer: DeviceId,
        decision: MembershipDecisionV2,
    },
    /// 把本机当前历史送给该对端核对。
    SynchronizeHistory { peer: DeviceId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledLedgerWork {
    pub work: LedgerWork,
    /// 最早可以执行的时间。
    pub due_at_ms: i64,
    /// 未完成时设备更新不能显示为完成。移除通知与离开到期不阻塞设备更新。
    pub blocks_device_update: bool,
}

impl MembershipLedger {
    /// 全部未完成待办。执行器只执行已到期项，并按其余项的最早时间安排唤醒。
    pub fn outstanding_work(
        &self,
        now_ms: i64,
    ) -> Result<Vec<ScheduledLedgerWork>, LedgerTransitionError> {
        let mut work = Vec::new();
        let mut effects: Vec<&UnfinishedMemberEffect> = self.effects.values().collect();
        effects.sort_by_key(|effect| (self.history.depth(effect.event_id()), effect.event_id()));
        work.extend(effects.into_iter().map(|effect| ScheduledLedgerWork {
            work: LedgerWork::AdvanceEffect(effect.clone()),
            due_at_ms: now_ms,
            blocks_device_update: true,
        }));
        for (peer, link) in &self.peers {
            match link {
                PeerLink::Departing(departing) => {
                    if now_ms < departing.expires_at_ms() {
                        work.push(ScheduledLedgerWork {
                            work: LedgerWork::DeliverRemovalNotice {
                                peer: *peer,
                                notice: departing.notice().clone(),
                            },
                            due_at_ms: now_ms,
                            blocks_device_update: false,
                        });
                    }
                    work.push(ScheduledLedgerWork {
                        work: LedgerWork::EndDeparture { peer: *peer },
                        due_at_ms: departing.expires_at_ms(),
                        blocks_device_update: false,
                    });
                }
                PeerLink::Member(member) => {
                    if let Some(decision) = member.outgoing_decision() {
                        work.push(ScheduledLedgerWork {
                            work: LedgerWork::DeliverDecision {
                                peer: *peer,
                                decision: decision.clone(),
                            },
                            due_at_ms: now_ms,
                            blocks_device_update: true,
                        });
                    }
                }
            }
        }
        if self.local_status() == LedgerMemberStatus::Active {
            let current = self.history.current_position()?;
            let mut peers: Vec<DeviceId> = self
                .active_peer_devices()?
                .into_iter()
                .filter(|peer| !self.effect_affects(peer))
                .filter(|peer| {
                    matches!(
                        self.peers.get(peer),
                        Some(PeerLink::Member(member)) if member.needs_history_sync(&current)
                    )
                })
                .collect();
            // 从上次轮转位置之后开始，避免排序靠前的对端长期占用每轮预算。
            if let Some(cursor) = &self.sync_cursor {
                let split = peers.partition_point(|peer| peer <= cursor);
                peers.rotate_left(split);
            }
            for peer in peers {
                if let Some(PeerLink::Member(member)) = self.peers.get(&peer) {
                    work.push(ScheduledLedgerWork {
                        due_at_ms: member.sync().due_at_ms(now_ms),
                        work: LedgerWork::SynchronizeHistory { peer },
                        blocks_device_update: true,
                    });
                }
            }
        }
        Ok(work)
    }
}
