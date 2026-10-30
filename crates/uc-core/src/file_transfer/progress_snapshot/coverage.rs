use super::super::{valid_progress_plan, ProgressIdentity, ProgressItem, RegistrationDescriptor};
use super::{
    ProgressSnapshotEffect as Effect, ProgressSnapshotInput as Input,
    ProgressSnapshotRefusal as Refusal, MAX_SNAPSHOT_PAGE_ITEMS,
};

/// 单一已授权任务的连续页覆盖；不保存业务载荷，也不把部分覆盖当作可采用的快照。
pub struct ProgressSnapshotCoverage {
    descriptor: RegistrationDescriptor<ProgressIdentity>,
    plan: Vec<ProgressItem>,
    revision: Option<u64>,
    next_offset: usize,
    sealed: bool,
}
impl ProgressSnapshotCoverage {
    pub fn new(
        descriptor: RegistrationDescriptor<ProgressIdentity>,
        plan: Vec<ProgressItem>,
    ) -> Result<Self, Refusal> {
        if !valid_progress_plan(&plan) || usize::from(descriptor.item_count) != plan.len() {
            return Err(Refusal::InvalidPlan);
        }
        Ok(Self {
            descriptor,
            plan,
            revision: None,
            next_offset: 0,
            sealed: false,
        })
    }
    pub fn apply(&mut self, input: Input) -> Effect {
        if self.sealed {
            return Effect::Refused(Refusal::Sealed);
        }
        let Input::Page {
            descriptor,
            revision,
            offset,
            total,
            items,
        } = input
        else {
            return self.refuse(Refusal::Incomplete);
        };
        let refused = if descriptor != self.descriptor {
            Some(Refusal::DescriptorChanged)
        } else if self.revision.is_some_and(|current| current != revision) {
            Some(Refusal::RevisionChanged)
        } else if usize::from(offset) != self.next_offset {
            Some(Refusal::WrongOffset)
        } else if usize::from(total) != self.plan.len() {
            Some(Refusal::WrongTotal)
        } else if items.is_empty() || items.len() > MAX_SNAPSHOT_PAGE_ITEMS {
            Some(Refusal::PageSize)
        } else if self
            .plan
            .get(self.next_offset..self.next_offset + items.len())
            != Some(items.as_slice())
        {
            Some(Refusal::WrongItem)
        } else {
            None
        };
        if let Some(reason) = refused {
            return self.refuse(reason);
        }
        self.revision = Some(revision);
        self.next_offset += items.len();
        if self.next_offset == self.plan.len() {
            self.sealed = true;
            Effect::Complete { revision }
        } else {
            Effect::Accepted {
                next_offset: self.next_offset as u16,
                revision,
            }
        }
    }
    fn refuse(&mut self, reason: Refusal) -> Effect {
        self.sealed = true;
        Effect::Refused(reason)
    }
}
