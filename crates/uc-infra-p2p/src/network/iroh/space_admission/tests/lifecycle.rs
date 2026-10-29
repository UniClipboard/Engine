//! 真实连接验证非成员资源隔离、取消与停止；不借成员目录授予权限。
use super::*;
use tokio::sync::Semaphore;

struct StalledCredentials {
    entered: Semaphore,
    active: Arc<AtomicUsize>,
    drained: Arc<Notify>,
}
struct CredentialWork {
    active: Arc<AtomicUsize>,
    drained: Arc<Notify>,
}
impl Drop for CredentialWork {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.drained.notify_one();
    }
}
#[async_trait]
impl SpaceAdmissionChannelCredentialPort for StalledCredentials {
    async fn resolve_initial(
        &self,
        _: InvitationId,
        _: SpaceAdmissionId,
    ) -> Result<SponsorOpaqueMaterial, SpaceAdmissionChannelCredentialError> {
        panic!("测试仅发送续接帧")
    }
    async fn load_continuation(
        &self,
        _: SpaceAdmissionId,
    ) -> Result<AdmissionContinuationCredential, SpaceAdmissionChannelCredentialError> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _work = CredentialWork {
            active: self.active.clone(),
            drained: self.drained.clone(),
        };
        self.entered.add_permits(1);
        std::future::pending().await
    }
}

async fn stalled_continuation(
    sponsor: &Endpoint,
    joiner: &Endpoint,
    credentials: &StalledCredentials,
) -> (
    Connection,
    iroh::endpoint::SendStream,
    iroh::endpoint::RecvStream,
) {
    let connection = connect(joiner, sponsor.addr()).await.expect("连接");
    let (mut send, receive) = open_stream(&connection).await.expect("开流");
    write_typed(
        &mut send,
        FrameKind::ContinuationHello,
        &ContinuationHelloV1 {
            admission_id: *admission_id().as_bytes(),
            local_peer_id: *joiner.id().as_bytes(),
            remote_peer_id: *sponsor.id().as_bytes(),
            nonce: [1; 32],
            request_digest: [0; 32],
            mac: vec![0; 32],
        },
        AUTH_FRAME_LIMIT,
    )
    .await
    .expect("续接帧");
    tokio::time::timeout(Duration::from_secs(3), credentials.entered.acquire())
        .await
        .expect("进入凭据读取")
        .expect("信号")
        .forget();
    (connection, send, receive)
}

#[tokio::test]
async fn one_nonmember_endpoint_cannot_exhaust_admission_and_shutdown_cancels_authentication() {
    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let flooder = bound_endpoint().await;
    let other = bound_endpoint().await;
    let endpoint = Arc::new(HangingLoopbackEndpoint {
        calls: AtomicUsize::new(0),
        entered: Notify::new(),
    });
    let credentials = Arc::new(StalledCredentials {
        entered: Semaphore::new(0),
        active: Arc::new(AtomicUsize::new(0)),
        drained: Arc::new(Notify::new()),
    });
    let transport = Arc::new(IrohSpaceAdmissionTransport::new(sponsor.clone()));
    let handler = Arc::new(
        IrohSpaceAdmissionHandler::new(transport.clone(), endpoint.clone(), credentials.clone())
            .expect("handler"),
    );
    let router = Router::builder((*sponsor).clone())
        .accept(SPACE_ADMISSION_ALPN, handler.clone())
        .spawn();
    let first = stalled_continuation(&sponsor, &flooder, &credentials).await;
    let second = stalled_continuation(&sponsor, &flooder, &credentials).await;
    let excess = connect(&flooder, sponsor.addr()).await.expect("第三条连接");
    tokio::time::timeout(Duration::from_secs(3), excess.closed())
        .await
        .expect("单端额度拒绝");
    let admitted_other = stalled_continuation(&sponsor, &other, &credentials).await;
    assert_eq!(credentials.active.load(Ordering::SeqCst), 3);
    assert_eq!(
        endpoint.calls.load(Ordering::SeqCst),
        0,
        "未认证不能处理业务"
    );

    tokio::time::timeout(Duration::from_secs(3), handler.shutdown())
        .await
        .expect("关闭不等待认证截止");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let changed = credentials.drained.notified();
            if credentials.active.load(Ordering::SeqCst) == 0 {
                break;
            }
            changed.await;
        }
        first.0.closed().await;
        second.0.closed().await;
        admitted_other.0.closed().await;
    })
    .await
    .expect("认证 future 与连接全部退出");
    router.shutdown().await.expect("router shutdown");
    flooder.close().await;
    other.close().await;
    sponsor.close().await;
}

#[derive(Debug)]
struct SilentSponsor {
    entered: Arc<Notify>,
    closed: Arc<Notify>,
}
impl ProtocolHandler for SilentSponsor {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        // 读到首帧才能证明客户端持有交换租约并进入网络等待。
        let (_send, mut receive) = connection
            .accept_bi()
            .await
            .map_err(AcceptError::from_err)?;
        read_raw_with_limit(&mut receive, AUTH_FRAME_LIMIT)
            .await
            .map_err(AcceptError::from_err)?;
        self.entered.notify_one();
        connection.closed().await;
        self.closed.notify_one();
        Ok(())
    }
}

#[tokio::test]
async fn dropping_initial_authentication_closes_its_managed_connection() {
    let sponsor = bound_endpoint().await;
    wait_for_direct_addrs(&sponsor).await;
    let joiner = bound_endpoint().await;
    let entered = Arc::new(Notify::new());
    let closed = Arc::new(Notify::new());
    let router = Router::builder((*sponsor).clone())
        .accept(
            SPACE_ADMISSION_ALPN,
            SilentSponsor {
                entered: entered.clone(),
                closed: closed.clone(),
            },
        )
        .spawn();
    let transport = IrohSpaceAdmissionTransport::new(joiner.clone());
    let invitation = InvitationId::from_bytes([7; 32]).expect("邀请标识");
    let route = SpaceAdmissionRoute::from_bytes(
        encode_space_admission_route(&sponsor.addr(), Some(invitation)).expect("路由编码"),
    )
    .expect("路由");
    let derived = SpaceAdmissionAuth::derive_password_equivalent(b"cancel-test", invitation);
    let password = AdmissionEncryptedPasswordEquivalent::from_bytes(derived.as_bytes().to_vec())
        .expect("凭据");
    let mut authentication = Box::pin(transport.establish_initial(
        admission_id(),
        AdmissionAttemptTimeline::start(1_000).expect("时间线"),
        &route,
        &password,
    ));
    tokio::time::timeout(Duration::from_secs(3), async {
        tokio::select! {
            _ = entered.notified() => {},
            _ = &mut authentication => panic!("认证仍应等待"),
        }
    })
    .await
    .expect("初次认证进入等待");
    drop(authentication);
    tokio::time::timeout(Duration::from_secs(3), closed.notified())
        .await
        .expect("丢弃认证关闭连接");
    transport.manager.shutdown().await.expect("准入排空");
    router.shutdown().await.expect("router shutdown");
    joiner.close().await;
    sponsor.close().await;
}
