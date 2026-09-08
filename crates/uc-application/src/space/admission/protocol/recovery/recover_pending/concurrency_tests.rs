use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use uc_core::membership::{
    AdmissionContinuationCredential, AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding,
    SpaceAdmissionEnvelopeV1, SpaceAdmissionId, SpaceAdmissionRoute,
};

use super::{
    AdmissionRecoveryTrigger, AuthenticatedAdmissionExchangePort, AuthenticatedAdmissionReply,
    SpaceAdmissionTransportError, SpaceAdmissionTransportPort,
};
use crate::space::admission::protocol::test_support::{
    ProtocolEvent, SpaceAdmissionProtocolTestPair,
};
use crate::space::admission::protocol::{
    ResolveJoinerInvitationError, ResolveJoinerInvitationPort,
};
use crate::space::admission::{CurrentJoinStatus, JoinSpaceInput};
use uc_core::membership::AdmissionShortInvitationCode;
use uc_core::pairing::invitation::FullInvitation;

#[derive(Clone, Copy)]
enum BlockAt {
    Authentication,
    Reply,
}

#[derive(Default)]
struct NetworkBarrier {
    entered: Notify,
    release: Notify,
}

impl NetworkBarrier {
    async fn wait(&self) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}

struct DelayedTransport {
    inner: Arc<dyn SpaceAdmissionTransportPort>,
    barrier: Arc<NetworkBarrier>,
    block_at: BlockAt,
}

struct DelayedExchange {
    inner: Box<dyn AuthenticatedAdmissionExchangePort>,
    barrier: Arc<NetworkBarrier>,
}

#[async_trait]
impl SpaceAdmissionTransportPort for DelayedTransport {
    async fn establish_initial(
        &self,
        id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        password: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        if matches!(self.block_at, BlockAt::Authentication) {
            self.barrier.wait().await;
        }
        let exchange = self.inner.establish_initial(id, route, password).await?;
        if matches!(self.block_at, BlockAt::Reply) {
            Ok(Box::new(DelayedExchange {
                inner: exchange,
                barrier: Arc::clone(&self.barrier),
            }))
        } else {
            Ok(exchange)
        }
    }

    async fn resume(
        &self,
        id: SpaceAdmissionId,
        route: &SpaceAdmissionRoute,
        peer: AdmissionPeerBinding,
        credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        Ok(Box::new(DelayedExchange {
            inner: self.inner.resume(id, route, peer, credential).await?,
            barrier: Arc::clone(&self.barrier),
        }))
    }
}

#[async_trait]
impl AuthenticatedAdmissionExchangePort for DelayedExchange {
    fn peer_binding(&self) -> AdmissionPeerBinding {
        self.inner.peer_binding()
    }

    fn take_newly_established_continuation(&mut self) -> Option<AdmissionContinuationCredential> {
        self.inner.take_newly_established_continuation()
    }

    async fn exchange(
        self: Box<Self>,
        request: &SpaceAdmissionEnvelopeV1,
    ) -> Result<AuthenticatedAdmissionReply, SpaceAdmissionTransportError> {
        self.barrier.wait().await;
        self.inner.exchange(request).await
    }
}

fn join_input() -> JoinSpaceInput {
    JoinSpaceInput {
        invitation_code: uc_core::pairing::InvitationCode::new("offline-recovery"),
        device_name: Some("Test device".to_string()),
        passphrase: uc_core::crypto::domain::Passphrase::new("test-passphrase"),
        preserve_unreadable_history: false,
    }
}

async fn assert_offline_recovery_does_not_block_local_actions(block_at: BlockAt) {
    let mut pair = SpaceAdmissionProtocolTestPair::receiving_commit().await;
    let started = pair.joiner().start_join(join_input()).await.unwrap();
    if matches!(block_at, BlockAt::Reply) {
        pair.joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged)
            .await;
    }
    let barrier = Arc::new(NetworkBarrier::default());
    pair.joiner_mut().recovery.transport = Arc::new(DelayedTransport {
        inner: Arc::clone(&pair.joiner().recovery.transport),
        barrier: Arc::clone(&barrier),
        block_at,
    });
    let CurrentJoinStatus::Pending { join_id, .. } = started.status else {
        panic!("join must start pending");
    };

    let recovery = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::Startup);
    let local_actions = async {
        barrier.entered.notified().await;
        let pending_transition = tokio::time::timeout(
            Duration::from_millis(250),
            pair.joiner().has_pending_space_transition(),
        )
        .await
        .expect("startup query must not wait for the offline sponsor")
        .unwrap();
        assert!(!pending_transition);

        let cancelled = tokio::time::timeout(
            Duration::from_millis(250),
            pair.joiner().cancel_join(join_id),
        )
        .await
        .expect("local cancellation must not wait for the network")
        .unwrap();
        assert!(matches!(
            cancelled,
            CurrentJoinStatus::Pending {
                cancel_requested: true,
                ..
            }
        ));
        barrier.release.notify_one();
    };
    let (report, ()) = tokio::join!(recovery, local_actions);
    assert_eq!(
        report.deferred_count, 1,
        "late reply must lose the version check"
    );
    let saved = pair.take_created_join();
    match block_at {
        BlockAt::Authentication => assert_eq!(
            saved.rejection_reason(),
            Some(uc_core::membership::SpaceAdmissionRejectionReason::Cancelled)
        ),
        BlockAt::Reply => assert!(saved.is_cancelling()),
    }
    assert!(!pair.events().contains(&ProtocolEvent::JoinerSavedCommitted));
}

#[tokio::test]
async fn offline_authentication_does_not_block_startup_or_cancellation() {
    assert_offline_recovery_does_not_block_local_actions(BlockAt::Authentication).await;
}

#[tokio::test]
async fn late_commit_reply_cannot_overwrite_local_cancellation() {
    assert_offline_recovery_does_not_block_local_actions(BlockAt::Reply).await;
}

struct DelayedResolver {
    inner: Arc<dyn ResolveJoinerInvitationPort>,
    barrier: Arc<NetworkBarrier>,
}

#[async_trait]
impl ResolveJoinerInvitationPort for DelayedResolver {
    async fn resolve_once(
        &self,
        code: &AdmissionShortInvitationCode,
    ) -> Result<FullInvitation, ResolveJoinerInvitationError> {
        self.barrier.wait().await;
        self.inner.resolve_once(code).await
    }
}

#[tokio::test]
async fn overlapping_recovery_does_not_consume_the_short_code_twice() {
    let mut pair = SpaceAdmissionProtocolTestPair::short_invitation().await;
    let barrier = Arc::new(NetworkBarrier::default());
    pair.joiner_mut().joiner.resolve_invitation = Arc::new(DelayedResolver {
        inner: Arc::clone(&pair.joiner().joiner.resolve_invitation),
        barrier: Arc::clone(&barrier),
    });
    let mut input = join_input();
    input.invitation_code = uc_core::pairing::InvitationCode::new("short-once");
    pair.joiner().start_join(input).await.unwrap();
    let first = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::Startup);
    let concurrent = async {
        barrier.entered.notified().await;
        let second = pair
            .joiner()
            .recover_pending(AdmissionRecoveryTrigger::StateChanged);
        tokio::pin!(second);
        assert!(tokio::time::timeout(Duration::from_millis(25), &mut second)
            .await
            .is_err());
        barrier.release.notify_one();
        second.await
    };
    let (first, second) = tokio::join!(first, concurrent);
    assert_eq!(first.advanced_count, 2);
    assert_eq!(second.advanced_count, 1);
    assert_eq!(
        pair.events()
            .iter()
            .filter(|event| **event == ProtocolEvent::JoinerInvitationResolutionRequested)
            .count(),
        1
    );
}
