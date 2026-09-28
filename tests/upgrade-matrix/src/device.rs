//! 一台测试设备：资料目录由场景租约持有，跨版本进程复用；安全存储由测试台在进程之间保存。

use std::time::Duration;

use serde_json::{Value, json};
use std::path::PathBuf;
use tokio::time::{Instant, sleep};

use uc_testkit::{FailureKind, ScenarioFailure};

use crate::{
    catalog::Point,
    cell::{CallErrors, CellRun},
    fixture::{Content, content_digest},
    host::{HostProcess, Started, StderrTail},
};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// 配对、互通与收敛的产品期限上限；超过即记为产品超时。
pub(crate) const CONVERGE_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) fn failure(kind: FailureKind, condition: &'static str) -> ScenarioFailure {
    ScenarioFailure::new(kind, condition)
}

/// 宿主启动的公开结果。
pub(crate) enum Launch {
    Ready { startup: Value },
    Refused { reply: Value },
}

pub(crate) struct Device {
    pub(crate) label: &'static str,
    pub(crate) name: &'static str,
    root: PathBuf,
    secure_storage: Option<Value>,
    host: Option<HostProcess>,
    pub(crate) point: Option<Point>,
    pub(crate) id: Option<String>,
    capabilities: Vec<String>,
    stderr: StderrTail,
    errors: CallErrors,
}

impl Device {
    pub(crate) fn new(
        run: &mut CellRun,
        label: &'static str,
        name: &'static str,
    ) -> Result<Self, ScenarioFailure> {
        let root = run.profile_dir(label)?;
        let stderr = run.stderr_tail(label);
        let errors = run.call_errors();
        Ok(Self {
            label,
            name,
            root,
            secure_storage: None,
            host: None,
            point: None,
            id: None,
            capabilities: Vec::new(),
            stderr,
            errors,
        })
    }

    /// 设备资料目录；旧版快照在首次启动前装入这里。
    pub(crate) fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// 首次启动前预置宿主安全存储，对应旧版在系统钥匙串中留下的条目。
    pub(crate) fn preload_secure_storage(&mut self, storage: Value) -> Result<(), ScenarioFailure> {
        if self.host.is_some() || self.secure_storage.is_some() {
            return Err(failure(
                FailureKind::FixtureInvalid,
                "secure-storage-already-present",
            ));
        }
        self.secure_storage = Some(storage);
        Ok(())
    }

    /// 以指定版本打开同一资料目录；无法在本地网络运行的旧版本记为环境不可用。
    pub(crate) async fn launch(
        &mut self,
        run: &mut CellRun,
        point: &Point,
    ) -> Result<Launch, ScenarioFailure> {
        if self.host.is_some() {
            return Err(failure(
                FailureKind::FixtureInvalid,
                "device-already-running",
            ));
        }
        let mut request = json!({
            "root": self.root,
            "rendezvous": run.rendezvous(),
            "relay": false,
            "app_version": point.app_version,
        });
        if let Some(storage) = &self.secure_storage {
            request["secure_storage"] = storage.clone();
        }
        run.record_event("host-starting");
        let started = HostProcess::start(point, request, self.stderr.clone()).await?;
        let reply = match &started {
            Started::Ready { reply, .. } | Started::Refused { reply } => reply.clone(),
        };
        run.record_host(self.label, point, &reply)?;
        if reply["error"] == "local_network_unavailable" {
            return Err(failure(
                FailureKind::EnvironmentUnavailable,
                "local-network-unavailable",
            ));
        }
        self.point = Some(point.clone());
        self.capabilities = reply["host"]["capabilities"]
            .as_array()
            .map(|names| {
                names
                    .iter()
                    .filter_map(|name| name.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        match started {
            Started::Ready { host, reply } => {
                self.host = Some(host);
                run.record_event("host-ready");
                Ok(Launch::Ready {
                    startup: reply["startup"].clone(),
                })
            }
            Started::Refused { reply } => {
                // 启动失败前 Engine 可能已改写安全存储；保存它，下一次启动才与真实宿主一致。
                if reply["secure_storage"].is_object() {
                    self.secure_storage = Some(reply["secure_storage"].clone());
                }
                run.record_event("host-refused");
                Ok(Launch::Refused { reply })
            }
        }
    }

    /// 打开资料并要求启动成功；启动升级进度可用时要求其终态为就绪。
    pub(crate) async fn start(
        &mut self,
        run: &mut CellRun,
        point: &Point,
    ) -> Result<Value, ScenarioFailure> {
        match self.launch(run, point).await? {
            Launch::Ready { startup } => {
                if !startup.is_null() && startup["state"] != "ready" {
                    return Err(failure(FailureKind::ProductInvariant, "startup-not-ready"));
                }
                Ok(startup)
            }
            Launch::Refused { .. } => {
                Err(failure(FailureKind::ProductInvariant, "host-start-failed"))
            }
        }
    }

    pub(crate) async fn stop(&mut self, run: &mut CellRun) -> Result<(), ScenarioFailure> {
        let Some(host) = self.host.take() else {
            return Ok(());
        };
        self.secure_storage = Some(host.stop().await?);
        run.record_event("host-stopped");
        Ok(())
    }

    /// 记录公开操作返回的错误码，供 `cell.json` 判定。
    pub(crate) fn record_error(&self, command: &str, reply: &Value) {
        if let Ok(mut errors) = self.errors.lock() {
            errors.push(json!({
                "device": self.label,
                "point": self.point.as_ref().map(|point| point.id.clone()),
                "command": command,
                "code": reply["code"],
            }));
        }
    }

    pub(crate) fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|name| name == capability)
    }

    fn host(&mut self) -> Result<&mut HostProcess, ScenarioFailure> {
        self.host
            .as_mut()
            .ok_or_else(|| failure(FailureKind::FixtureInvalid, "device-not-running"))
    }

    pub(crate) async fn call(
        &mut self,
        command: &str,
        fields: Value,
        condition: &'static str,
    ) -> Result<Value, ScenarioFailure> {
        let mut request = fields;
        if request.is_null() {
            request = json!({});
        }
        request["command"] = json!(command);
        let reply = self.host()?.raw(request).await?;
        match reply.get("ok") {
            Some(value) => Ok(value.clone()),
            None => {
                self.record_error(command, &reply);
                Err(failure(FailureKind::ProductInvariant, condition))
            }
        }
    }

    pub(crate) async fn raw(
        &mut self,
        command: &str,
        fields: Value,
    ) -> Result<Value, ScenarioFailure> {
        let mut request = fields;
        if request.is_null() {
            request = json!({});
        }
        request["command"] = json!(command);
        self.host()?.raw(request).await
    }

    pub(crate) async fn observe(&mut self) -> Result<Value, ScenarioFailure> {
        self.call("observe", Value::Null, "observe-failed").await
    }

    /// 取得可比较状态；会话未就绪时先以同一口令解锁，证明 Space 可解锁。
    pub(crate) async fn unlocked_observation(&mut self) -> Result<Value, ScenarioFailure> {
        let observed = self.observe().await?;
        if observed["encryption"]["session_ready"] == true {
            return Ok(observed);
        }
        self.call("unlock", Value::Null, "space-unlock-failed")
            .await?;
        let observed = self.observe().await?;
        if observed["encryption"]["session_ready"] != true {
            return Err(failure(FailureKind::ProductInvariant, "space-not-unlocked"));
        }
        Ok(observed)
    }

    pub(crate) async fn create_space(&mut self) -> Result<Value, ScenarioFailure> {
        let created = self
            .call(
                "create",
                json!({ "name": self.name }),
                "create-space-failed",
            )
            .await?;
        self.id = created["device"].as_str().map(str::to_owned);
        Ok(created)
    }

    pub(crate) async fn capture(&mut self, content: &Content) -> Result<(), ScenarioFailure> {
        let captured = self
            .call("capture", content.capture_request(), "capture-failed")
            .await?;
        if captured["entry"].is_null() {
            return Err(failure(
                FailureKind::ProductInvariant,
                "capture-created-no-entry",
            ));
        }
        Ok(())
    }

    /// 在期限内等待本机历史出现指定内容。
    pub(crate) async fn wait_for_content(
        &mut self,
        digest: &str,
        condition: &'static str,
    ) -> Result<Value, ScenarioFailure> {
        let deadline = Deadline::new(condition);
        loop {
            let observed = self.observe().await?;
            if history_contains(&observed, digest) {
                return Ok(observed);
            }
            deadline.tick().await?;
        }
    }

    pub(crate) fn local_id(observed: &Value) -> Option<String> {
        observed["local_device"]["id"].as_str().map(str::to_owned)
    }
}

pub(crate) fn history_contains(observed: &Value, digest: &str) -> bool {
    observed["history"].as_array().is_some_and(|entries| {
        entries
            .iter()
            .any(|entry| entry["content"]["bytes"] == digest)
    })
}

/// 轮询期限：条件未成立时等待下一次检查，超过产品期限记为产品超时；每次检查都经公开操作。
pub(crate) struct Deadline {
    at: Instant,
    condition: &'static str,
}

impl Deadline {
    pub(crate) fn new(condition: &'static str) -> Self {
        Self {
            at: Instant::now() + CONVERGE_TIMEOUT,
            condition,
        }
    }

    pub(crate) async fn tick(&self) -> Result<(), ScenarioFailure> {
        if Instant::now() >= self.at {
            return Err(failure(FailureKind::ProductTimeout, self.condition));
        }
        sleep(POLL_INTERVAL).await;
        Ok(())
    }
}

/// 写入代表性内容与同步偏好，并确认历史中确实出现每项内容。
pub(crate) async fn write_representative_content(
    device: &mut Device,
    contents: &[Content],
) -> Result<(), ScenarioFailure> {
    for content in contents {
        device.capture(content).await?;
    }
    let updated = device
        .call(
            "sync_preference",
            json!({ "sync_on_restore": true }),
            "sync-preference-update-failed",
        )
        .await?;
    if updated.get("Rejected").is_some() {
        return Err(failure(
            FailureKind::ProductInvariant,
            "sync-preference-rejected",
        ));
    }
    let observed = device.unlocked_observation().await?;
    for content in contents {
        if !history_contains(&observed, &content_digest(content)) {
            return Err(failure(
                FailureKind::ProductInvariant,
                "written-content-missing",
            ));
        }
    }
    if observed["settings"]["sync_on_restore"] != true {
        return Err(failure(
            FailureKind::ProductInvariant,
            "sync-preference-not-saved",
        ));
    }
    Ok(())
}
