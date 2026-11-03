//! 有界交互进程：调用方串行请求，退出时必须显式等待回收。
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{fs::File, path::Path, process::Stdio, time::Duration};
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::mpsc,
    task::JoinHandle,
    time::timeout,
};

#[derive(Debug, Error)]
#[error("host operation failed")]
pub struct HostOperationFailure {
    pub code: Option<u64>,
}

pub struct InteractiveProcess {
    child: Child,
    input: Option<ChildStdin>,
    replies: mpsc::Receiver<Result<Value>>,
    reader: JoinHandle<()>,
}
impl InteractiveProcess {
    pub fn spawn(binary: &Path, stderr: Option<&Path>) -> Result<Self> {
        let stderr = match stderr {
            Some(path) => Stdio::from(File::create(path)?),
            None => Stdio::null(),
        };
        let mut child = Command::new(binary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .kill_on_drop(true)
            .spawn()?;
        let input = child.stdin.take().context("child input missing")?;
        let output = child.stdout.take().context("child output missing")?;
        let (sender, replies) = mpsc::channel(1);
        let reader = tokio::spawn(async move {
            let mut output = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let result: Result<Option<Value>> = async {
                    let count = (&mut output)
                        .take(65537)
                        .read_until(b'\n', &mut bytes)
                        .await?;
                    if count == 0 {
                        return Ok(None);
                    }
                    if count > 65536 {
                        bail!("control response exceeds bound");
                    }
                    let value: Value = serde_json::from_slice(&bytes)?;
                    Ok(Some(value))
                }
                .await;
                match result {
                    Ok(None) => break,
                    Ok(Some(value)) if value["uc_connectivity"] != 1 => continue,
                    Ok(Some(value)) => {
                        if sender.try_send(Ok(value)).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.try_send(Err(error));
                        break;
                    }
                }
            }
        });
        Ok(Self {
            child,
            input: Some(input),
            replies,
            reader,
        })
    }
    pub fn pid(&self) -> Result<u32> {
        self.child.id().context("child already exited")
    }
    pub async fn exchange(&mut self, value: Value) -> Result<Value> {
        timeout(Duration::from_secs(30), async {
            let mut bytes = serde_json::to_vec(&value)?;
            if bytes.len() > 65536 {
                bail!("control request exceeds bound");
            }
            bytes.push(b'\n');
            let input = self.input.as_mut().context("child input closed")?;
            input.write_all(&bytes).await?;
            input.flush().await?;
            self.replies.recv().await.context("control stream closed")?
        })
        .await?
    }
    pub async fn request(&mut self, value: Value) -> Result<Value> {
        timeout(Duration::from_secs(30), async {
            loop {
                let response = self.exchange(value.clone()).await?;
                if let Some(ok) = response.get("ok") {
                    return Ok(ok.clone());
                }
                let code = response["code"].as_u64();
                let retry = matches!(value["command"].as_str(), Some("invite" | "eligibility"))
                    && matches!(code, Some(1103 | 1211 | 1212));
                if !retry {
                    return Err(HostOperationFailure { code }.into());
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await?
    }
    pub async fn stop(&mut self) -> Result<()> {
        self.request(serde_json::json!({"command":"shutdown"}))
            .await?;
        self.input.take();
        let status = timeout(Duration::from_secs(30), self.child.wait()).await??;
        if !status.success() {
            bail!("host exit failed");
        }
        Ok(())
    }
    pub async fn kill(&mut self) -> Result<()> {
        self.child.kill().await?;
        self.child.wait().await?;
        Ok(())
    }
}
impl Drop for InteractiveProcess {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
