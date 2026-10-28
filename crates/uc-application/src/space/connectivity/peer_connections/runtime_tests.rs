//! 成员范围读取完成与通知之间的额度交接。
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{watch, Semaphore};

use super::{read_scope, CurrentSpaceMemberScope, CurrentSpaceMemberScopePort, ScopeWork};
use crate::space::membership::CurrentSpaceMemberScopeError;

struct Scope(watch::Sender<()>);

#[async_trait]
impl CurrentSpaceMemberScopePort for Scope {
    async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
        Ok(CurrentSpaceMemberScope {
            revision: 1,
            local_member_active: true,
            usable_peer_device_ids: vec![],
            paused_peer_devices: vec![],
        })
    }
    fn subscribe_changes(&self) -> watch::Receiver<()> {
        self.0.subscribe()
    }
}

#[tokio::test]
async fn completed_scope_read_releases_capacity_before_waking_next_refresh() {
    let scope = Arc::new(Scope(watch::channel(()).0));
    let gate = Arc::new(Semaphore::new(0));
    let work = Arc::new(Mutex::new(ScopeWork {
        after_send: Some(gate.clone()),
        ..Default::default()
    }));
    read_scope(scope.clone(), work.clone()).await.unwrap();
    let next = read_scope(scope, work.clone()).await;
    gate.add_permits(1);
    let tasks = {
        let mut work = work.lock().unwrap();
        std::mem::take(&mut work.tasks)
    };
    tasks.join_all().await;
    assert!(next.is_ok(), "已完成读取的额度不得在通知后继续阻塞刷新");
}
