use tokio::sync::broadcast;
use uc_core::membership::{SpaceMembershipState, WorkspaceSnapshot};

/// 当前 Space 成员状态的进程内失效通知。
#[derive(Clone)]
pub(crate) struct SpaceMembershipStateEvents {
    sender: broadcast::Sender<WorkspaceSnapshot>,
}

impl SpaceMembershipStateEvents {
    pub(crate) fn new() -> Self {
        let (sender, _) = broadcast::channel(64);
        Self { sender }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<WorkspaceSnapshot> {
        self.sender.subscribe()
    }

    pub(crate) fn publish(&self, state: &SpaceMembershipState) {
        let _ = self.sender.send(state.snapshot());
    }
}
