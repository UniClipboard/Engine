//! Independent Engine test host. Control traffic uses inherited pipes only.

use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use uc_engine::observability::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, ProcessObservabilityRuntime,
};
use uc_engine::{
    ConnectivityOpportunity, CreateSpaceInput, Engine, EngineConfig, EngineEvent,
    HistoryEntryInput, HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory,
    HostClipboard, HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle,
    HostFileMetadata, HostSecureStorage, JoinSpaceInput, ListHistoryEntriesInput, Operation,
    OperationResult, RemoveMemberInput, SecretString, SendTextInput,
};

#[derive(Clone, Default)]
struct SecureStorage(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl HostSecureStorage for SecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.0.lock().map_err(|_| unavailable())?.get(key).cloned())
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.0
            .lock()
            .map_err(|_| unavailable())?
            .insert(key.into(), value.to_vec());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.0.lock().map_err(|_| unavailable())?.remove(key);
        Ok(())
    }
}

fn unavailable() -> HostCapabilityError {
    HostCapabilityError::new(
        HostCapabilityErrorCategory::Unavailable,
        "test host unavailable",
    )
}

struct Clipboard;
impl HostClipboard for Clipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 0,
            representations: vec![],
        })
    }
    fn write(&self, _: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}
struct Files;
impl HostFileAccess for Files {
    fn metadata(&self, _: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        Err(unavailable())
    }
    fn read_chunk(
        &self,
        _: &HostFileHandle,
        _: u64,
        _: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        Err(unavailable())
    }
    fn write_chunk(&self, _: &HostFileHandle, _: u64, _: &[u8]) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
    fn finish_write(&self, _: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key].as_str().context("missing test command field")
}

fn respond(mut value: Value) -> Result<()> {
    value["uc_connectivity"] = json!(1);
    let mut output = io::stdout().lock();
    serde_json::to_writer(&mut output, &value)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

const PASSPHRASE: &str = "connection-recovery-synthetic-passphrase";

#[cfg(feature = "current-engine")]
async fn shutdown_engine(engine: &Engine) -> Result<()> {
    engine.shutdown_until_complete().await?;
    Ok(())
}

#[cfg(not(feature = "current-engine"))]
async fn shutdown_engine(engine: &Engine) -> Result<()> {
    engine.shutdown(Duration::from_secs(15)).await?;
    Ok(())
}

async fn operation(engine: &Engine, request: &Value) -> Result<Value> {
    let command = string(request, "command")?;
    #[cfg(feature = "current-engine")]
    if command == "connections" {
        let uc_engine::DevOperationResult::PeerReachabilityConnections {
            incoming,
            outgoing,
            registered_tasks,
            admitted_transports,
        } = engine
            .execute_dev(uc_engine::DevOperation::QueryPeerReachabilityConnections)
            .await?
        else {
            bail!("connection counts expected")
        };
        return Ok(
            json!({ "incoming": incoming, "outgoing": outgoing, "registered_tasks": registered_tasks, "admitted_transports": admitted_transports }),
        );
    }
    #[cfg(feature = "current-engine")]
    if command == "suppress_opportunities" {
        engine
            .execute_dev(uc_engine::DevOperation::SuppressConnectivityOpportunities {
                suppressed: request["suppressed"]
                    .as_bool()
                    .context("missing suppression flag")?,
            })
            .await?;
        return Ok(json!(true));
    }
    if command == "suspend" {
        engine.suspend().await?;
        return Ok(json!(true));
    }
    if command == "resume" {
        engine.resume().await?;
        return Ok(json!(true));
    }
    if command == "shutdown" {
        shutdown_engine(engine).await?;
        return Ok(json!(true));
    }
    let op = match command {
        "create" => Operation::CreateSpace(CreateSpaceInput {
            device_name: Some(string(request, "name")?.into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }),
        "invite" => Operation::IssueInvitation,
        "join" => Operation::JoinSpace(JoinSpaceInput {
            invitation_code: string(request, "invitation")?.into(),
            device_name: Some(string(request, "name")?.into()),
            passphrase: SecretString::new(PASSPHRASE),
            preserve_unreadable_history: false,
        }),
        "relay_config" => Operation::UpdateSettings(Box::new(uc_engine::SettingsPatch {
            network: Some(uc_engine::NetworkSettingsPatch {
                allow_relay_fallback: Some(true),
                custom_relay_urls: Some(vec![string(request, "url")?.into()]),
                ..Default::default()
            }),
            ..Default::default()
        })),
        "setup" => Operation::QuerySetupState,
        "peers" => Operation::QueryPeerConnections,
        "eligibility" => Operation::QueryDeviceGroupChoices,
        "opportunity" => Operation::NotifyConnectivityOpportunity {
            reason: ConnectivityOpportunity::NetworkChanged,
        },
        "recover" => Operation::RecoverNetwork,
        "remove" => Operation::RemoveMember(RemoveMemberInput {
            device_id: string(request, "peer")?.into(),
        }),
        "send" => Operation::SendText(SendTextInput {
            text: string(request, "text")?.into(),
            target_devices: vec![string(request, "peer")?.into()],
        }),
        "history" => Operation::ListHistoryEntries(ListHistoryEntriesInput {
            limit: 100,
            offset: 0,
        }),
        "entry" => Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: string(request, "entry")?.into(),
        }),
        _ => bail!("unknown test command"),
    };
    Ok(match engine.execute(op).await? {
        OperationResult::SpaceCreated {
            space_id,
            self_device_id,
            ..
        } => json!({ "space": space_id, "device": self_device_id }),
        OperationResult::InvitationIssued {
            full_invitation, ..
        } => json!({ "invitation": full_invitation }),
        OperationResult::JoinSpace(status) => serde_json::to_value(status)?,
        OperationResult::SetupState(state) => {
            json!({ "space_id": state.space_id, "has_completed": state.has_completed })
        }
        OperationResult::PeerConnections(peers) => serde_json::to_value(peers)?,
        OperationResult::DeviceGroupChoices(choices) => serde_json::to_value(choices)?,
        OperationResult::EntrySent(report) => serde_json::to_value(report)?,
        OperationResult::HistoryEntries(entries) => serde_json::to_value(entries)?,
        OperationResult::HistoryEntry(entry) => serde_json::to_value(entry)?,
        _ => json!(true),
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let mut lines = io::stdin().lock().lines();
    let start: Value = serde_json::from_str(&lines.next().context("missing startup request")??)?;
    let root = PathBuf::from(string(&start, "root")?);
    let os = if cfg!(target_os = "linux") {
        OperatingSystem::Linux
    } else {
        OperatingSystem::Macos
    };
    let observability = ProcessObservabilityRuntime::install(
        ObservabilityConfig::new(ObservabilityResource::new(
            env!("CARGO_PKG_VERSION"),
            DeploymentEnvironment::Test,
            os,
            "test",
        )?)
        .with_local_logs(LocalLogConfig::new(root.join("logs"))),
    )?
    .handle();
    let storage = SecureStorage::default();
    if let Some(value) = start.get("secure_storage") {
        *storage
            .0
            .lock()
            .map_err(|_| anyhow!("storage unavailable"))? = serde_json::from_value(value.clone())?;
    }
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(storage.clone()),
        Box::new(Clipboard),
        Box::new(Files),
    );
    let config = EngineConfig::new("1.1.0")
        .with_rendezvous_base_url(string(&start, "rendezvous")?)
        .with_test_relay_fallback(start["relay"].as_bool().unwrap_or(false));
    #[cfg(feature = "current-engine")]
    let config = match start["bind_port"].as_u64() {
        Some(port) => config.with_test_iroh_bind_port(
            u16::try_from(port).context("test bind port is out of range")?,
        ),
        None => config,
    };
    let (engine, mut events) = Engine::start(config, host).await?;
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let event_task = tokio::spawn({
        let recorded = recorded.clone();
        async move {
            while let Some(event) = events.next().await {
                let value = match event {
                    EngineEvent::PeerPresenceChanged(change) => {
                        json!({ "kind": "peer", "peer": change.device_id, "state": change.state })
                    }
                    EngineEvent::NetworkRecoveryChanged(state) => {
                        json!({ "kind": "recovery", "state": state })
                    }
                    EngineEvent::RefreshRequired {
                        reason: uc_engine::RefreshReason::ConsumerLagged,
                    } => json!({ "kind": "lost_events" }),
                    EngineEvent::Fatal { error } | EngineEvent::LifecycleFailed { error, .. } => {
                        json!({ "kind": "host_failed", "code": error.code() })
                    }
                    _ => continue,
                };
                let Ok(mut recorded) = recorded.lock() else {
                    return;
                };
                if recorded.len() >= 4096 {
                    recorded.clear();
                    recorded.push(json!({ "kind": "lost_events" }));
                    return;
                }
                recorded.push(value);
            }
        }
    });
    respond(json!({ "ready": true, "version": env!("CARGO_PKG_VERSION") }))?;
    let mut shut_down = false;
    for line in lines {
        let request: Value = serde_json::from_str(&line?)?;
        let command = string(&request, "command")?;
        let result = match command {
            "flush" => {
                observability.force_flush(Duration::from_secs(2));
                Ok(json!(true))
            }
            "events" => recorded
                .lock()
                .map(|mut events| json!(std::mem::take(&mut *events)))
                .map_err(|_| anyhow!("events unavailable")),
            "secure_storage" => storage
                .0
                .lock()
                .map(|values| json!(*values))
                .map_err(|_| anyhow!("storage unavailable")),
            _ => operation(&engine, &request).await,
        };
        let response = match result {
            Ok(value) => json!({ "ok": value }),
            Err(error) => {
                json!({ "error": "operation_failed", "code": error.downcast_ref::<uc_engine::EngineError>().map(uc_engine::EngineError::code) })
            }
        };
        respond(response.clone())?;
        if command == "shutdown" {
            shut_down = response.get("ok").is_some();
            break;
        }
    }
    if !shut_down {
        shutdown_engine(&engine).await?;
    }
    event_task.await?;
    observability.shutdown(Duration::from_secs(2));
    Ok(())
}
