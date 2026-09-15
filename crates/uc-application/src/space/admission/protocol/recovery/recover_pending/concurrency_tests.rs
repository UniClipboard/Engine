use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use uc_core::membership::{
    AdmissionAttemptTimeline, AdmissionContinuationCredential,
    AdmissionEncryptedPasswordEquivalent, AdmissionPeerBinding, SpaceAdmissionEnvelopeV1,
    SpaceAdmissionId, SpaceAdmissionRoute,
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
    delay_next_reply: Arc<AtomicBool>,
}

struct DelayedFirstTransport {
    barrier: Arc<NetworkBarrier>,
    delay_next: AtomicBool,
}

struct DelayedExchange {
    inner: Box<dyn AuthenticatedAdmissionExchangePort>,
    barrier: Arc<NetworkBarrier>,
    delay_next_reply: Arc<AtomicBool>,
}

#[async_trait]
impl SpaceAdmissionTransportPort for DelayedTransport {
    async fn establish_initial(
        &self,
        id: SpaceAdmissionId,
        attempt_timeline: AdmissionAttemptTimeline,
        route: &SpaceAdmissionRoute,
        password: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        if matches!(self.block_at, BlockAt::Authentication) {
            self.barrier.wait().await;
        }
        let exchange = self
            .inner
            .establish_initial(id, attempt_timeline, route, password)
            .await?;
        if matches!(self.block_at, BlockAt::Reply) {
            Ok(Box::new(DelayedExchange {
                inner: exchange,
                barrier: Arc::clone(&self.barrier),
                delay_next_reply: Arc::clone(&self.delay_next_reply),
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
            delay_next_reply: Arc::clone(&self.delay_next_reply),
        }))
    }
}

#[async_trait]
impl SpaceAdmissionTransportPort for DelayedFirstTransport {
    async fn establish_initial(
        &self,
        _id: SpaceAdmissionId,
        _attempt_timeline: AdmissionAttemptTimeline,
        _route: &SpaceAdmissionRoute,
        _password: &AdmissionEncryptedPasswordEquivalent,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        if self.delay_next.swap(false, Ordering::SeqCst) {
            self.barrier.wait().await;
        }
        Err(SpaceAdmissionTransportError::Deferred)
    }

    async fn resume(
        &self,
        _id: SpaceAdmissionId,
        _route: &SpaceAdmissionRoute,
        _peer: AdmissionPeerBinding,
        _credential: &AdmissionContinuationCredential,
    ) -> Result<Box<dyn AuthenticatedAdmissionExchangePort>, SpaceAdmissionTransportError> {
        Err(SpaceAdmissionTransportError::Deferred)
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
        if self.delay_next_reply.swap(false, Ordering::SeqCst) {
            self.barrier.wait().await;
        }
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
    let started = pair
        .joiner()
        .start_join_at(join_input(), 1_000)
        .await
        .unwrap();
    let barrier = Arc::new(NetworkBarrier::default());
    pair.joiner_mut().recovery.transport = Arc::new(DelayedTransport {
        inner: Arc::clone(&pair.joiner().recovery.transport),
        barrier: Arc::clone(&barrier),
        block_at,
        delay_next_reply: Arc::new(AtomicBool::new(matches!(block_at, BlockAt::Reply))),
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
        assert!(matches!(cancelled, CurrentJoinStatus::Terminated { .. }));
    };
    let (report, ()) = tokio::time::timeout(Duration::from_millis(500), async {
        tokio::join!(recovery, local_actions)
    })
    .await
    .expect("local cancellation must stop the in-flight network attempt");
    assert_eq!(report.deferred_count, 0);
    let saved = pair.take_created_join();
    assert_eq!(
        saved.termination_reason(),
        Some(uc_core::membership::SpaceAdmissionTerminationReason::Cancelled)
    );
    assert!(!pair.events().contains(&ProtocolEvent::JoinerSavedCommitted));
}

#[tokio::test]
async fn offline_authentication_does_not_block_startup_or_cancellation() {
    assert_offline_recovery_does_not_block_local_actions(BlockAt::Authentication).await;
}

#[tokio::test]
async fn offline_reply_wait_is_stopped_by_local_cancellation() {
    assert_offline_recovery_does_not_block_local_actions(BlockAt::Reply).await;
}

#[tokio::test]
async fn a_replacement_join_stops_the_previous_network_attempt_and_recovers_immediately() {
    let mut pair = SpaceAdmissionProtocolTestPair::receiving_commit().await;
    pair.joiner()
        .start_join_at(join_input(), 1_000)
        .await
        .unwrap();
    let barrier = Arc::new(NetworkBarrier::default());
    pair.joiner_mut().recovery.transport = Arc::new(DelayedFirstTransport {
        barrier: Arc::clone(&barrier),
        delay_next: AtomicBool::new(true),
    });
    pair.set_next_join_identity(0x21, 0x22);

    let recovery = pair
        .joiner()
        .recover_pending(AdmissionRecoveryTrigger::Startup);
    let replace = async {
        barrier.entered.notified().await;
        let mut input = join_input();
        input.invitation_code = uc_core::pairing::InvitationCode::new("replacement-join");
        pair.joiner()
            .start_join_at(input, 1_000)
            .await
            .expect("replacement join should be saved");
    };
    let (report, ()) = tokio::time::timeout(Duration::from_millis(500), async {
        tokio::join!(recovery, replace)
    })
    .await
    .expect("replacement join must stop the previous network attempt");

    assert!(pair.superseded_previous_join());
    assert_eq!(report.deferred_count, 1);
    assert_eq!(pair.saved_join().admission_id().as_bytes(), &[0x21; 32]);
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
    pair.joiner().start_join_at(input, 1_000).await.unwrap();
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
