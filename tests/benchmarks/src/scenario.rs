use crate::{metrics::monotonic_ns, report, SCENARIO_VERSION};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    process::{Child, Command},
    time::{sleep, timeout},
};
use uc_testkit::{mount_rendezvous, HostOperationFailure, InteractiveProcess};

struct Host {
    process: InteractiveProcess,
    collector: Option<Child>,
    root: PathBuf,
    startup_ns: u64,
}
impl Host {
    async fn start(
        binary: &Path,
        root: PathBuf,
        url: &str,
        output: Option<&Path>,
        stderr: Option<&Path>,
        interval: u64,
        storage: Value,
    ) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let started = Instant::now();
        let mut process = InteractiveProcess::spawn(binary, stderr)?;
        let collector = if let Some(path) = output {
            Some(
                Command::new(env::current_exe()?)
                    .args([
                        "collect",
                        "--pid",
                        &process.pid()?.to_string(),
                        "--interval-ms",
                        &interval.to_string(),
                        "--output",
                    ])
                    .arg(path)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .spawn()?,
            )
        } else {
            None
        };
        let mut request = json!({"root":root,"rendezvous":url,"relay":false,"legacy_group_updates":false,"benchmark_stream":true});
        if !storage.is_null() {
            request["secure_storage"] = storage;
        }
        let ready = process.exchange(request).await?;
        ensure!(ready["ready"] == true, "host startup failed");
        Ok(Self {
            process,
            collector,
            root,
            startup_ns: started.elapsed().as_nanos() as u64,
        })
    }
    async fn stop_collector(&mut self) -> Result<()> {
        if let Some(mut collector) = self.collector.take() {
            collector.stdin.take();
            ensure!(
                timeout(Duration::from_secs(5), collector.wait())
                    .await??
                    .success(),
                "collector failed"
            );
        }
        Ok(())
    }
}
async fn wait_for_samples(path: &Path, deadline: Duration) -> Result<()> {
    let started = Instant::now();
    loop {
        let samples = fs::read_to_string(path)
            .map(|content| content.lines().count())
            .unwrap_or(0);
        if samples >= 2 {
            return Ok(());
        }
        ensure!(started.elapsed() < deadline, "collector sample deadline");
        sleep(Duration::from_millis(50)).await;
    }
}
async fn wait_ready(host: &mut InteractiveProcess, join: bool) -> Result<()> {
    let start = Instant::now();
    loop {
        let status = host.request(json!({"command":"eligibility"})).await?;
        let trust = &status["device_trust"];
        if trust["space_device_update"]["phase"] == "completed"
            && (!join || trust["current_join"]["status"] == "active")
        {
            return Ok(());
        }
        ensure!(
            start.elapsed() < Duration::from_secs(30),
            "pairing deadline"
        );
        sleep(Duration::from_millis(100)).await;
    }
}
async fn wait_connected(host: &mut InteractiveProcess) -> Result<()> {
    let started = Instant::now();
    loop {
        let peers = host.request(json!({"command":"peers"})).await?;
        if peers.as_array().is_some_and(|peers| {
            peers
                .iter()
                .any(|peer| peer["connected"] == true && peer["is_paired"] == true)
        }) {
            return Ok(());
        }
        ensure!(
            started.elapsed() < Duration::from_secs(30),
            "peer connection deadline"
        );
        sleep(Duration::from_millis(100)).await;
    }
}
async fn page(host: &mut InteractiveProcess, offset: usize, limit: usize) -> Result<Vec<Value>> {
    let value = host
        .request(json!({"command":"benchmark_text_page","offset":offset,"limit":limit}))
        .await?;
    value.as_array().cloned().context("text page missing")
}
async fn verify_history(host: &mut InteractiveProcess, expected: &[Value]) -> Result<()> {
    let mut actual = Vec::new();
    loop {
        let next = page(host, actual.len(), 32).await?;
        if next.is_empty() {
            break;
        }
        actual.extend(next);
        ensure!(actual.len() <= expected.len(), "unexpected history entries");
    }
    let mut expected = expected.to_vec();
    actual.sort_by_key(Value::to_string);
    expected.sort_by_key(Value::to_string);
    ensure!(actual == expected, "history mismatch");
    Ok(())
}
pub async fn run(binary: &Path, output: &Path, root: &Path, kind: &str, fault: &str) -> Result<()> {
    let result = workload(binary, output, root, kind, fault).await;
    if let Err(error) = &result {
        let categories = [
            "host operation failed",
            "host startup failed",
            "pairing deadline",
            "peer connection deadline",
            "text synchronization deadline",
            "history mismatch",
            "unexpected history entries",
            "collector failed",
            "control stream closed",
            "host exit failed",
            "injected host crash",
        ];
        let reason = categories
            .iter()
            .find(|reason| error.chain().any(|source| source.to_string() == **reason))
            .copied()
            .unwrap_or("workload_failed");
        report::write(
            &output.join("work.json"),
            &json!({"passed":false,"failure":reason,"engine_code":error.downcast_ref::<HostOperationFailure>().and_then(|failure|failure.code)}),
        )?;
    }
    result
}
async fn workload(
    binary: &Path,
    output: &Path,
    root: &Path,
    kind: &str,
    fault: &str,
) -> Result<()> {
    let rendezvous = mount_rendezvous().await;
    let interval = if matches!(kind, "disconnected" | "connected") {
        1000
    } else {
        50
    };
    let mut a = Host::start(
        binary,
        root.join("a"),
        &rendezvous.uri(),
        Some(&output.join("a.jsonl")),
        Some(&output.join("a.stderr.log")),
        interval,
        Value::Null,
    )
    .await?;
    let mut b = Host::start(
        binary,
        root.join("b"),
        &rendezvous.uri(),
        Some(&output.join("b.jsonl")),
        Some(&output.join("b.stderr.log")),
        interval,
        Value::Null,
    )
    .await?;
    if fault == "timeout" {
        sleep(Duration::from_secs(120)).await;
    }
    if fault == "host-crash" {
        report::write(&output.join("fault.json"), &json!({"injected":fault}))?;
        b.process.kill().await?;
        bail!("injected host crash");
    }
    if fault == "collector-crash" {
        report::write(&output.join("fault.json"), &json!({"injected":fault}))?;
        if let Some(child) = b.collector.as_mut() {
            child.kill().await?;
        }
    }
    let mut expected = Vec::new();
    let mut windows = Vec::new();
    let started = Instant::now();
    windows.push(json!({"phase":"setup_start","monotonic_ns":monotonic_ns()}));
    if kind != "startup" {
        a.process
            .request(json!({"command":"create","name":"Benchmark A"}))
            .await?;
        if kind != "disconnected" {
            let invite = a.process.request(json!({"command":"invite"})).await?;
            b.process.request(json!({"command":"join","name":"Benchmark B","invitation":invite["invitation"]})).await?;
            wait_ready(&mut a.process, false).await?;
            wait_ready(&mut b.process, true).await?;
            wait_connected(&mut a.process).await?;
            wait_connected(&mut b.process).await?;
            for host in [&mut a, &mut b] {
                host.process.request(json!({"command":"sync_preference","auto_sync_enabled":true,"sync_on_restore":false})).await?;
            }
        } else {
            b.process
                .request(json!({"command":"create","name":"Benchmark B"}))
                .await?;
        }
        windows.push(json!({"phase":"warmup_start","monotonic_ns":monotonic_ns()}));
        if kind != "smoke" {
            // macOS 分配器可能暂留已释放的 KDF 页面；初始化样本仍完整保留。
            sleep(Duration::from_secs(90)).await;
        }
        windows.push(json!({"phase":"work_start","elapsed_ns":started.elapsed().as_nanos() as u64,"monotonic_ns":monotonic_ns()}));
        if matches!(kind, "text" | "smoke") {
            let count = if kind == "smoke" { 2 } else { 200 };
            for index in 0..count {
                let size = if index < count / 2 { 100 } else { 10240 };
                let mut text = format!("benchmark-374-v1-{index:06}-");
                text.extend(std::iter::repeat_n('x', size - text.len()));
                let item =
                    json!({"sha256":format!("{:x}",Sha256::digest(text.as_bytes())),"size":size});
                a.process.request(json!({"command":"benchmark_inject","kind":"text","text":text,"observed_at_ms":1700000000000_i64+index as i64})).await?;
                let deadline = Instant::now();
                loop {
                    let received = page(&mut b.process, 0, 1).await?;
                    if received.first() == Some(&item) {
                        break;
                    }
                    ensure!(
                        deadline.elapsed() < Duration::from_secs(30),
                        "text synchronization deadline"
                    );
                    sleep(Duration::from_millis(100)).await;
                }
                expected.push(item);
            }
            verify_history(&mut b.process, &expected).await?;
        } else {
            sleep(Duration::from_secs(60)).await;
        }
        windows.push(json!({"phase":"work_end","elapsed_ns":started.elapsed().as_nanos() as u64,"monotonic_ns":monotonic_ns()}));
        if kind == "text" {
            sleep(Duration::from_secs(30)).await;
        }
        if kind != "disconnected" {
            wait_ready(&mut a.process, false).await?;
            wait_ready(&mut b.process, true).await?;
            wait_connected(&mut a.process).await?;
            wait_connected(&mut b.process).await?;
        }
    }
    windows.push(json!({"phase":"sampling_end","monotonic_ns":monotonic_ns()}));
    wait_for_samples(&output.join("a.jsonl"), Duration::from_secs(10)).await?;
    wait_for_samples(&output.join("b.jsonl"), Duration::from_secs(10)).await?;
    a.stop_collector().await?;
    b.stop_collector().await?;
    if !expected.is_empty() {
        let storage = b
            .process
            .request(json!({"command":"secure_storage"}))
            .await?;
        b.process.stop().await?;
        let mut restarted = Host::start(
            binary,
            b.root.clone(),
            &rendezvous.uri(),
            None,
            Some(&output.join("b-restart.stderr.log")),
            interval,
            storage,
        )
        .await?;
        restarted
            .process
            .request(json!({"command":"unlock"}))
            .await?;
        verify_history(&mut restarted.process, &expected).await?;
        restarted.process.stop().await?;
    } else {
        b.process.stop().await?;
    }
    a.process.stop().await?;
    report::write(
        &output.join("work.json"),
        &json!({"passed":true,"scenario_version":SCENARIO_VERSION,"warmup_seconds":if matches!(kind,"startup"|"smoke") {0} else {90},"a_startup_ns":a.startup_ns,"b_startup_ns":b.startup_ns,"verified_entries":expected.len(),"restart_verified":!expected.is_empty(),"windows":windows,"interval_ms":interval}),
    )?;
    Ok(())
}

#[cfg(unix)]
pub async fn supervised_run(
    binary: &Path,
    output: &Path,
    root: &Path,
    kind: &str,
    fault: &str,
) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use tokio::io::unix::AsyncFd;
    ensure!(
        unsafe { libc::getpgrp() } == unsafe { libc::getpid() },
        "worker requires a private process group"
    );
    // 标准输入只由本控制器拥有；非阻塞管道避免退出时遗留阻塞读取线程。
    let fd = unsafe { OwnedFd::from_raw_fd(0) };
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    ensure!(
        flags >= 0
            && unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == 0,
        "worker pipe setup failed"
    );
    // fd 由 OwnedFd 唯一拥有，注册期间不会替换或关闭。
    let pipe = unsafe { AsyncFd::register(fd)? };
    async fn byte(pipe: &AsyncFd<OwnedFd>) -> Result<bool> {
        loop {
            let mut guard = pipe.readable().await?;
            let read = guard.try_io(|inner| {
                let mut value = [0u8];
                let count = unsafe { libc::read(inner.as_raw_fd(), value.as_mut_ptr().cast(), 1) };
                if count < 0 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(count == 1)
                }
            });
            if let Ok(value) = read {
                return Ok(value?);
            }
        }
    }
    ensure!(byte(&pipe).await?, "supervisor exited before admission");
    tokio::select! {
        result = run(binary, output, root, kind, fault) => result,
        _ = byte(&pipe) => {
            // 监督器已丢失；本组只有本次生成的控制器、宿主和采样器。
            unsafe { libc::kill(0, libc::SIGKILL); }
            bail!("supervisor lost")
        }
    }
}
#[cfg(not(unix))]
pub async fn supervised_run(_: &Path, _: &Path, _: &Path, _: &str, _: &str) -> Result<()> {
    bail!("macOS required")
}
