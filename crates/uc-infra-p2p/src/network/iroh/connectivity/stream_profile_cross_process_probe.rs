//! 独立进程验证真实恢复连接过载后的两端回收。
use super::*;
use std::process::Stdio;
use tokio::net::{UnixListener, UnixStream};
use tokio::process::Command;

const CHILD_SOCKET: &str = "UC_BRANCH_RECOVERY_CAPACITY_CHILD_SOCKET";
const TEST_NAME: &str = "network::iroh::connectivity::stream::profile_probe::limits_probe::cross_process::recovery_capacity_shutdown_releases_connections_across_processes";

#[cfg(target_os = "linux")]
fn open_file_descriptors() -> anyhow::Result<usize> {
    Ok(std::fs::read_dir("/proc/self/fd")?.count())
}

#[cfg(target_os = "linux")]
fn file_descriptor_categories() -> anyhow::Result<[usize; 4]> {
    let mut counts = [0; 4];
    for entry in std::fs::read_dir("/proc/self/fd")? {
        let target = match std::fs::read_link(entry?.path()) {
            Ok(target) => target,
            Err(_) => continue,
        };
        let target = target.to_string_lossy();
        let category = if target.starts_with("socket:") {
            0
        } else if target.starts_with("pipe:") {
            1
        } else if target.starts_with("anon_inode:") {
            2
        } else {
            3
        };
        counts[category] += 1;
    }
    Ok(counts)
}

#[cfg(target_os = "linux")]
async fn wait_for_file_descriptors(
    role: &'static str,
    baseline: (usize, [usize; 4]),
) -> anyhow::Result<()> {
    let result = timeout(BUDGET, async {
        loop {
            if open_file_descriptors()? <= baseline.0 {
                return anyhow::Ok(());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    match result {
        Ok(result) => result?,
        Err(source) => {
            return Err(anyhow::Error::new(source).context(format!(
                "{role} 文件描述符未回落: baseline={baseline:?}, current={}, socket_pipe_anon_other={:?}",
                open_file_descriptors()?,
                file_descriptor_categories()?
            )))
        }
    }
    Ok(())
}

async fn send_address(socket: &mut UnixStream, address: &EndpointAddr) -> anyhow::Result<()> {
    let bytes = postcard::to_stdvec(address)?;
    socket.write_u32(u32::try_from(bytes.len())?).await?;
    socket.write_all(&bytes).await?;
    Ok(())
}

async fn receive_address(socket: &mut UnixStream) -> anyhow::Result<EndpointAddr> {
    let length = socket.read_u32().await?;
    anyhow::ensure!(length <= 4096, "测试端点地址过长");
    let mut bytes = vec![0; usize::try_from(length)?];
    socket.read_exact(&mut bytes).await?;
    Ok(postcard::from_bytes(&bytes)?)
}

async fn receive_signal(socket: &mut UnixStream, expected: u8) -> anyhow::Result<()> {
    anyhow::ensure!(
        socket.read_u8().await? == expected,
        "测试进程同步信号不匹配"
    );
    Ok(())
}

async fn client_process(path: &std::ffi::OsStr) -> anyhow::Result<()> {
    let mut socket = UnixStream::connect(path).await?;
    #[cfg(target_os = "linux")]
    let baseline = (open_file_descriptors()?, file_descriptor_categories()?);
    let client = endpoint().await?;
    send_address(&mut socket, &client.addr()).await?;
    let server = receive_address(&mut socket).await?;
    let mut clients = Vec::new();
    let mut handles = Vec::new();
    for _ in 0..8 {
        let connection = client
            .connect(server.clone(), MEMBERSHIP_BRANCH_RECOVERY_ALPN)
            .await?;
        handles.push(connection.weak_handle());
        let (mut send, receive) = connection.open_bi().await?;
        send.write_all(&[1]).await?;
        clients.push((connection, send, receive));
        receive_signal(&mut socket, 10).await?;
    }
    let rejected = client
        .connect(server.clone(), MEMBERSHIP_BRANCH_RECOVERY_ALPN)
        .await?;
    handles.push(rejected.weak_handle());
    timeout(BUDGET, rejected.closed()).await?;
    receive_signal(&mut socket, 1).await?;
    let replacement = client
        .connect(server, MEMBERSHIP_BRANCH_RECOVERY_ALPN)
        .await?;
    handles.push(replacement.weak_handle());
    let (mut send, receive) = replacement.open_bi().await?;
    send.write_all(&[1]).await?;
    clients.push((replacement, send, receive));
    receive_signal(&mut socket, 2).await?;
    drop(clients);
    drop(rejected);
    client.close().await;
    drop(client);
    assert_eq!(handles.len(), 10);
    timeout(BUDGET, async {
        while handles.iter().any(|handle| handle.upgrade().is_some()) {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    #[cfg(target_os = "linux")]
    wait_for_file_descriptors("client", baseline).await?;
    socket.write_u8(3).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_capacity_shutdown_releases_connections_across_processes() -> anyhow::Result<()> {
    timeout(Duration::from_secs(90), async {
        if let Some(path) = std::env::var_os(CHILD_SOCKET) {
            return client_process(&path).await;
        }
        let directory = tempfile::tempdir()?;
        let socket_path = directory.path().join("control.sock");
        let listener = UnixListener::bind(&socket_path)?;
        let mut child = Command::new(std::env::current_exe()?)
            .args(["--exact", TEST_NAME, "--nocapture"])
            .env(CHILD_SOCKET, &socket_path)
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let (mut socket, _) = timeout(BUDGET, listener.accept()).await??;
        let client = receive_address(&mut socket).await?;
        #[cfg(target_os = "linux")]
        let baseline = (open_file_descriptors()?, file_descriptor_categories()?);
        let server = endpoint().await?;
        let peers = Arc::new(Directory {
            peers: vec![
                (DeviceId::new("client"), client),
                (DeviceId::new("server"), server.addr()),
            ],
            allowed: AtomicBool::new(true),
        });
        let manager = ConnectivityManager::new(
            Arc::new(IrohConnectivityBackend::new(
                server.clone(),
                peers.clone(),
                None,
                ConnectivityProfile::MembershipBranchRecovery,
            )),
            Arc::new(BranchRecoveryAdmission::new(
                peers.clone(),
                peers,
                Arc::new(Sha256IdentityFingerprintFactory),
                AdmittedConnections::default(),
            )),
            ConnectivityPolicy {
                outbound_per_peer: 1,
                inbound_per_connection: 1,
                inbound_per_session: 8,
                reject_excess_inbound: true,
                waiters_per_peer: 4,
                waiters_per_session: 16,
                flights_per_session: 4,
                supervise_local_work: true,
                ..ConnectivityPolicy::default()
            },
        );
        let (events, mut received) = mpsc::channel(16);
        let handlers = Arc::new(AtomicUsize::new(0));
        let handles = Arc::new(Mutex::new(Vec::new()));
        let router = Router::builder((*server).clone())
            .accept(
                MEMBERSHIP_BRANCH_RECOVERY_ALPN,
                RecoveryCapacityCapture {
                    manager: manager.clone(),
                    events,
                    active: handlers.clone(),
                    native_handles: handles.clone(),
                },
            )
            .spawn();
        send_address(&mut socket, &server.addr()).await?;
        let mut active = Vec::new();
        for _ in 0..8 {
            active.push(
                timeout(BUDGET, received.recv())
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("缺少入站结果"))??,
            );
            socket.write_u8(10).await?;
        }
        let rejected = timeout(BUDGET, received.recv())
            .await?
            .ok_or_else(|| anyhow::anyhow!("缺少过载结果"))?;
        assert!(matches!(rejected, Err(ConnectivityError::Busy(_))));
        timeout(BUDGET, async {
            while handlers.load(Ordering::SeqCst) != 8 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(active.pop());
        timeout(BUDGET, async {
            while handlers.load(Ordering::SeqCst) != 7 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        socket.write_u8(1).await?;
        active.push(
            timeout(BUDGET, received.recv())
                .await?
                .ok_or_else(|| anyhow::anyhow!("缺少替换连接结果"))??,
        );
        manager.shutdown().await?;
        for (connection, _, _) in &active {
            timeout(BUDGET, connection.closed()).await?;
        }
        timeout(BUDGET, async {
            while handlers.load(Ordering::SeqCst) != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        drop(active);
        router.shutdown().await?;
        drop(router);
        server.close().await;
        drop(manager);
        drop(server);
        assert_eq!(handles.lock().unwrap().len(), 10);
        timeout(BUDGET, async {
            while handles
                .lock()
                .unwrap()
                .iter()
                .any(|handle| handle.upgrade().is_some())
            {
                tokio::task::yield_now().await;
            }
        })
        .await?;
        #[cfg(target_os = "linux")]
        wait_for_file_descriptors("server", baseline).await?;
        socket.write_u8(2).await?;
        receive_signal(&mut socket, 3).await?;
        anyhow::ensure!(
            timeout(BUDGET, child.wait()).await??.success(),
            "测试子进程失败"
        );
        Ok(())
    })
    .await?
}
