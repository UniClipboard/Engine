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
    OperationResult, RemoveMemberInput, SecretString, SendFilesInput, SendTextInput,
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
#[derive(Clone, Default)]
struct Files(Arc<Mutex<HashMap<String, ManagedFile>>>);

#[derive(Clone)]
struct ManagedFile {
    display_name: String,
    mime_type: Option<String>,
    bytes: Vec<u8>,
}

impl HostFileAccess for Files {
    fn metadata(&self, handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        let files = self.0.lock().map_err(|_| unavailable())?;
        let file = files.get(handle.as_str()).ok_or_else(invalid_handle)?;
        Ok(HostFileMetadata {
            display_name: file.display_name.clone(),
            size_bytes: file.bytes.len() as u64,
            mime_type: file.mime_type.clone(),
        })
    }
    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        let files = self.0.lock().map_err(|_| unavailable())?;
        let file = files.get(handle.as_str()).ok_or_else(invalid_handle)?;
        let start = usize::try_from(offset).map_err(|_| unavailable())?;
        if start >= file.bytes.len() {
            return Ok(Vec::new());
        }
        let end = start
            .saturating_add(max_bytes as usize)
            .min(file.bytes.len());
        Ok(file.bytes[start..end].to_vec())
    }
    fn write_chunk(&self, _: &HostFileHandle, _: u64, _: &[u8]) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
    fn finish_write(&self, _: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Err(unavailable())
    }
}

fn invalid_handle() -> HostCapabilityError {
    HostCapabilityError::new(
        HostCapabilityErrorCategory::InvalidHandle,
        "test file handle unavailable",
    )
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

#[cfg(feature = "current-engine")]
fn space_work_event_kind(value: &str) -> Result<uc_engine::DevSpaceWorkEventKind> {
    match value {
        "final_confirmation_connection_failed" => {
            Ok(uc_engine::DevSpaceWorkEventKind::FinalConfirmationConnectionFailed)
        }
        "final_confirmation_sponsor_committed" => {
            Ok(uc_engine::DevSpaceWorkEventKind::FinalConfirmationSponsorCommitted)
        }
        "final_confirmation_success_reply_dropped" => {
            Ok(uc_engine::DevSpaceWorkEventKind::FinalConfirmationSuccessReplyDropped)
        }
        "final_confirmation_retry_started" => {
            Ok(uc_engine::DevSpaceWorkEventKind::FinalConfirmationRetryStarted)
        }
        "final_confirmation_reply_received" => {
            Ok(uc_engine::DevSpaceWorkEventKind::FinalConfirmationReplyReceived)
        }
        "ordinary_member_update_started" => {
            Ok(uc_engine::DevSpaceWorkEventKind::OrdinaryMemberUpdateStarted)
        }
        "membership_history_sync_started" => {
            Ok(uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncStarted)
        }
        "membership_history_sync_retryable_failure" => {
            Ok(uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncRetryableFailure)
        }
        "membership_history_sync_needs_attention" => {
            Ok(uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncNeedsAttention)
        }
        "membership_history_sync_reply_received" => {
            Ok(uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncReplyReceived)
        }
        _ => bail!("unknown Space work event kind"),
    }
}

#[cfg(feature = "current-engine")]
fn space_work_event_json(event: uc_engine::DevSpaceWorkEvent) -> Value {
    let kind = match event.kind {
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationConnectionFailed => {
            "final_confirmation_connection_failed"
        }
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationSponsorCommitted => {
            "final_confirmation_sponsor_committed"
        }
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationSuccessReplyDropped => {
            "final_confirmation_success_reply_dropped"
        }
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationRetryStarted => {
            "final_confirmation_retry_started"
        }
        uc_engine::DevSpaceWorkEventKind::FinalConfirmationReplyReceived => {
            "final_confirmation_reply_received"
        }
        uc_engine::DevSpaceWorkEventKind::OrdinaryMemberUpdateStarted => {
            "ordinary_member_update_started"
        }
        uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncStarted => {
            "membership_history_sync_started"
        }
        uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncRetryableFailure => {
            "membership_history_sync_retryable_failure"
        }
        uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncNeedsAttention => {
            "membership_history_sync_needs_attention"
        }
        uc_engine::DevSpaceWorkEventKind::MembershipHistorySyncReplyReceived => {
            "membership_history_sync_reply_received"
        }
    };
    json!({ "sequence": event.sequence, "kind": kind })
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

async fn operation(engine: &Engine, files: &Files, request: &Value) -> Result<Value> {
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
    #[cfg(feature = "current-engine")]
    if command == "arm_complete_ack_failure" {
        let uc_engine::DevOperationResult::FinalConfirmationConnectionFailureArmed {
            after_sequence,
        } = engine
            .execute_dev(uc_engine::DevOperation::ArmFinalConfirmationConnectionFailure)
            .await?
        else {
            bail!("final confirmation failure arm result expected")
        };
        return Ok(json!({ "after_sequence": after_sequence }));
    }
    #[cfg(feature = "current-engine")]
    if command == "arm_complete_ack_success_reply_drop" {
        let uc_engine::DevOperationResult::FinalConfirmationSuccessReplyDropArmed {
            after_sequence,
        } = engine
            .execute_dev(uc_engine::DevOperation::ArmFinalConfirmationSuccessReplyDrop)
            .await?
        else {
            bail!("final confirmation success reply drop arm result expected")
        };
        return Ok(json!({ "after_sequence": after_sequence }));
    }
    #[cfg(feature = "current-engine")]
    if command == "arm_membership_history_failures" {
        let failure = match string(request, "failure")? {
            "retryable" => uc_engine::DevMembershipHistoryFailure::Retryable,
            "needs_attention" => uc_engine::DevMembershipHistoryFailure::NeedsAttention,
            _ => bail!("unknown membership history failure kind"),
        };
        let count = request["count"]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .context("missing or invalid membership history failure count")?;
        let uc_engine::DevOperationResult::MembershipHistoryFailuresArmed { after_sequence } =
            engine
                .execute_dev(uc_engine::DevOperation::ArmMembershipHistoryFailures {
                    failure,
                    count,
                })
                .await?
        else {
            bail!("membership history failures arm result expected")
        };
        return Ok(json!({ "after_sequence": after_sequence }));
    }
    #[cfg(feature = "current-engine")]
    if command == "clear_membership_history_failures" {
        let uc_engine::DevOperationResult::MembershipHistoryFailuresCleared { remaining } = engine
            .execute_dev(uc_engine::DevOperation::ClearMembershipHistoryFailures)
            .await?
        else {
            bail!("membership history failures clear result expected")
        };
        return Ok(json!({ "remaining": remaining }));
    }
    #[cfg(feature = "current-engine")]
    if command == "wait_space_work_event" {
        let after_sequence = request["after_sequence"]
            .as_u64()
            .context("missing Space work event sequence")?;
        let kind = space_work_event_kind(string(request, "kind")?)?;
        let uc_engine::DevOperationResult::SpaceWorkEvent(event) = engine
            .execute_dev(uc_engine::DevOperation::WaitForSpaceWorkEvent {
                after_sequence,
                kind,
            })
            .await?
        else {
            bail!("Space work event expected")
        };
        return Ok(space_work_event_json(event));
    }
    #[cfg(feature = "current-engine")]
    if command == "space_work_events" {
        let uc_engine::DevOperationResult::SpaceWorkEvents(events) = engine
            .execute_dev(uc_engine::DevOperation::QuerySpaceWorkEvents)
            .await?
        else {
            bail!("Space work events expected")
        };
        return Ok(Value::Array(
            events.into_iter().map(space_work_event_json).collect(),
        ));
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
        "send_file" => {
            let handle = string(request, "handle")?;
            files.0.lock().map_err(|_| unavailable())?.insert(
                handle.to_owned(),
                ManagedFile {
                    display_name: string(request, "display_name")?.to_owned(),
                    mime_type: request["mime_type"].as_str().map(str::to_owned),
                    bytes: string(request, "content")?.as_bytes().to_vec(),
                },
            );
            Operation::SendFiles(SendFilesInput {
                files: vec![HostFileHandle::new(handle)],
                target_devices: vec![string(request, "peer")?.into()],
            })
        }
        "history" => Operation::ListHistoryEntries(ListHistoryEntriesInput {
            limit: 100,
            offset: 0,
        }),
        "entry" => Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: string(request, "entry")?.into(),
        }),
        "read_file" => Operation::ReadEntryFile(HistoryEntryInput {
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
        OperationResult::EntryFileRead(resource) => serde_json::to_value(resource)?,
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
    let files = Files::default();
    let host = HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(storage.clone()),
        Box::new(Clipboard),
        Box::new(files.clone()),
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
            _ => operation(&engine, &files, &request).await,
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
