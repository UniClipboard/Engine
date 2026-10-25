//! 完整快照的原子合并；重用单项规则，任何冲突都不授权部分保存。
use super::super::{
    valid_progress_plan, ProgressItem, ProgressItemEffect, ProgressItemInput,
    ProgressItemProjection,
};
use super::{ProgressItemState, ProgressSnapshotRefusal};

pub enum ProgressSnapshotMerge {
    /// 不保存或发布变化，重复或落后的快照不能续期。
    Unchanged,
    /// 先一次保存全部新投影，再发布变化；不得逐项对外提交。
    Adopt(Vec<ProgressItemProjection>),
}

pub fn merge_progress_snapshot(
    plan: &[ProgressItem],
    current: &[Option<ProgressItemState>],
    incoming: &[ProgressItemState],
) -> Result<ProgressSnapshotMerge, ProgressSnapshotRefusal> {
    if !valid_progress_plan(plan) {
        return Err(ProgressSnapshotRefusal::InvalidPlan);
    }
    if current.len() != plan.len() || incoming.len() != plan.len() {
        return Err(ProgressSnapshotRefusal::WrongTotal);
    }
    let mut merged = Vec::with_capacity(plan.len());
    let mut changed = false;
    for ((item, old), next) in plan.iter().zip(current).zip(incoming) {
        if old.is_some_and(|old| old.item() != *item) || next.item() != *item {
            return Err(ProgressSnapshotRefusal::WrongItem);
        }
        let mut projection = ProgressItemProjection::default();
        if let Some(old) = old {
            if projection.apply(input(*old)) != ProgressItemEffect::Adopted {
                return Err(ProgressSnapshotRefusal::InvalidState);
            }
        }
        if match next {
            ProgressItemState::Active(value) => {
                value.total.is_some_and(|total| value.bytes > total)
            }
            ProgressItemState::Terminal(value) => {
                value.total.is_some_and(|total| value.bytes > total)
            }
        } {
            return Err(ProgressSnapshotRefusal::InvalidState);
        }
        match projection.apply(input(*next)) {
            ProgressItemEffect::Adopted => changed = true,
            ProgressItemEffect::Duplicate | ProgressItemEffect::Stale => {}
            ProgressItemEffect::Conflict => return Err(ProgressSnapshotRefusal::StateConflict),
            ProgressItemEffect::Invalid => return Err(ProgressSnapshotRefusal::InvalidState),
        }
        merged.push(projection);
    }
    Ok(if changed {
        ProgressSnapshotMerge::Adopt(merged)
    } else {
        ProgressSnapshotMerge::Unchanged
    })
}
fn input(value: ProgressItemState) -> ProgressItemInput {
    match value {
        ProgressItemState::Active(value) => ProgressItemInput::Update(value),
        ProgressItemState::Terminal(value) => ProgressItemInput::Terminal(value),
    }
}
