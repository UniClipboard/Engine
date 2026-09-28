//! 连接测试宿主进程驱动：stdin/stdout 逐行 JSON，stderr 保留有界尾部作为失败工件。

use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
    time::timeout,
};
use uc_testkit::{FailureKind, ScenarioFailure};

use crate::catalog::Point;

/// 单条宿主指令的保护期限；长于 Engine 自身操作期限，只用于识别卡死。
const REPLY_TIMEOUT: Duration = Duration::from_secs(130);
const EXIT_TIMEOUT: Duration = Duration::from_secs(60);
const STDERR_TAIL_BYTES: usize = 64 * 1024;

/// 宿主二进制位置：旧锚点在 `<anchors>/<rev>/bin`，当前源码在 `<target>/debug`。
pub(crate) fn binary(point: &Point) -> PathBuf {
    let target = std::env::var_os("UC_UPGRADE_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    // Windows 可执行文件带 `.exe`；缺少后缀会被误判为宿主缺失而跳过单元。
    let name = format!("uc-connectivity-host{}", std::env::consts::EXE_SUFFIX);
    match &point.engine_rev {
        Some(rev) => target
            .join("upgrade-anchors")
            .join(rev)
            .join("bin")
            .join(name),
        None => target.join("debug").join(name),
    }
}

/// 宿主启动结果：就绪，或宿主如实报告无法启动（启动失败或无法关闭外部网络）。
pub(crate) enum Started {
    Ready { host: HostProcess, reply: Value },
    Refused { reply: Value },
}

pub(crate) struct HostProcess {
    child: Child,
    input: ChildStdin,
    output: Lines<BufReader<ChildStdout>>,
}

pub(crate) type StderrTail = Arc<Mutex<Vec<u8>>>;

fn failure(kind: FailureKind, condition: &'static str) -> ScenarioFailure {
    ScenarioFailure::new(kind, condition)
}

impl HostProcess {
    pub(crate) async fn start(
        point: &Point,
        request: Value,
        stderr_tail: StderrTail,
    ) -> Result<Started, ScenarioFailure> {
        let binary = binary(point);
        if !binary.is_file() {
            return Err(failure(
                FailureKind::EnvironmentUnavailable,
                "anchor-host-missing",
            ));
        }
        let mut child = Command::new(&binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| failure(FailureKind::EnvironmentUnavailable, "host-spawn"))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| failure(FailureKind::DriverProtocol, "host-stdin"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| failure(FailureKind::DriverProtocol, "host-stdout"))?;
        if let Some(mut stderr) = child.stderr.take() {
            let tail = Arc::clone(&stderr_tail);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 4096];
                while let Ok(read) = stderr.read(&mut buffer).await {
                    if read == 0 {
                        break;
                    }
                    let Ok(mut tail) = tail.lock() else { break };
                    tail.extend_from_slice(&buffer[..read]);
                    let excess = tail.len().saturating_sub(STDERR_TAIL_BYTES);
                    tail.drain(..excess);
                }
            });
        }
        let mut host = Self {
            child,
            input,
            output: BufReader::new(output).lines(),
        };
        let reply = host.exchange(&request).await?;
        if reply["ready"] == true {
            Ok(Started::Ready { host, reply })
        } else {
            host.wait_exit().await?;
            Ok(Started::Refused { reply })
        }
    }

    async fn exchange(&mut self, request: &Value) -> Result<Value, ScenarioFailure> {
        let mut line = request.to_string();
        line.push('\n');
        self.input
            .write_all(line.as_bytes())
            .await
            .map_err(|_| failure(FailureKind::DriverProtocol, "host-exited-before-request"))?;
        self.input
            .flush()
            .await
            .map_err(|_| failure(FailureKind::DriverProtocol, "host-exited-before-request"))?;
        loop {
            let next = timeout(REPLY_TIMEOUT, self.output.next_line())
                .await
                .map_err(|_| failure(FailureKind::ProductTimeout, "host-reply-timeout"))?
                .map_err(|_| failure(FailureKind::DriverProtocol, "host-reply-read"))?;
            let Some(line) = next else {
                return Err(failure(
                    FailureKind::ProductInvariant,
                    "host-exited-before-reply",
                ));
            };
            let Ok(reply) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if reply["uc_connectivity"] == 1 {
                return Ok(reply);
            }
        }
    }

    /// 发送指令并返回完整回复（含 `ok` 或 `error`/`code`）。
    pub(crate) async fn raw(&mut self, request: Value) -> Result<Value, ScenarioFailure> {
        self.exchange(&request).await
    }

    /// 发送指令；产品返回错误时以 `condition` 记为产品失败。
    pub(crate) async fn call(
        &mut self,
        request: Value,
        condition: &'static str,
    ) -> Result<Value, ScenarioFailure> {
        let reply = self.exchange(&request).await?;
        match reply.get("ok") {
            Some(value) => Ok(value.clone()),
            None => Err(failure(FailureKind::ProductInvariant, condition)),
        }
    }

    /// 正常关闭：先取回安全存储，再请求关闭并确认进程以成功状态退出。
    pub(crate) async fn stop(mut self) -> Result<Value, ScenarioFailure> {
        let storage = self
            .call(
                json!({ "command": "secure_storage" }),
                "secure-storage-export",
            )
            .await?;
        self.call(json!({ "command": "shutdown" }), "host-shutdown")
            .await?;
        self.wait_exit().await?;
        Ok(storage)
    }

    async fn wait_exit(&mut self) -> Result<(), ScenarioFailure> {
        drop(self.input.shutdown().await);
        let status = timeout(EXIT_TIMEOUT, self.child.wait())
            .await
            .map_err(|_| failure(FailureKind::ProductTimeout, "host-exit-timeout"))?
            .map_err(|_| failure(FailureKind::DriverProtocol, "host-exit-wait"))?;
        if status.success() {
            Ok(())
        } else {
            Err(failure(FailureKind::ProductInvariant, "host-exit-status"))
        }
    }
}
