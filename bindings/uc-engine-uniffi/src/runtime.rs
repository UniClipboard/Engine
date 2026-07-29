use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use uc_engine::{
    ClipboardRestoreMode, ClipboardRestoreOutcome, CreateSpaceInput, Engine, EngineConfig,
    ExportEntryInput, HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory,
    HostClipboard, HostClipboardRepresentation, HostClipboardSnapshot, HostDirectories,
    HostFileAccess, HostFileHandle, HostFileMetadata, HostSecureStorage, JoinSpaceInput,
    ObserveClipboardChangeInput, Operation, OperationResult, QueryMemberRevocationInput,
    RecoverSessionInput, RemoveMemberInput, ResendEntryInput, RestoreClipboardInput, SecretString,
    SendFilesInput, SendImageInput, SendTextInput,
};
use zeroize::Zeroizing;

use crate::{
    BindingClipboardOrigin, BindingClipboardRepresentation, BindingClipboardRestoreMode,
    BindingClipboardRestoreOutcome, BindingClipboardSnapshot, BindingConfig, BindingEngineState,
    BindingError, BindingEvent, BindingFailure, BindingFileMetadata, BindingHost,
    BindingLifecycleAction, BindingOperationTerminal, BindingRefreshReason,
    BindingTransferDirection, HostBindingError,
};

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceCreated {
    pub space_id: String,
    pub self_device_id: String,
    pub identity_fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SessionRecovery {
    pub unlocked: bool,
    pub resumed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ActiveClipboard {
    pub entry_id: String,
    pub activated_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LocalDevice {
    pub device_id: String,
    pub display_name: String,
}

#[derive(Clone, PartialEq, Eq, uniffi::Record)]
pub struct InvitationIssued {
    pub invitation_code: String,
    pub expires_at_ms: i64,
    pub availability: InvitationAvailability,
}

impl std::fmt::Debug for InvitationIssued {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("InvitationIssued")
            .field("invitation_code", &"[REDACTED]")
            .field("expires_at_ms", &self.expires_at_ms)
            .field("availability", &self.availability)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum InvitationAvailability {
    CrossNetwork,
    SameLocalNetwork,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceJoined {
    pub sponsor_device_id: String,
    pub sponsor_identity_fingerprint: String,
    pub space_id: String,
    pub self_device_id: String,
    pub self_identity_fingerprint: String,
    pub migrated_records: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SendReport {
    pub entry_id: String,
    pub at_ms: i64,
    pub total_accepted: u64,
    pub total_duplicate: u64,
    pub total_offline: u64,
    pub total_errored: u64,
    pub total_pending: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PeerConnectionRefresh {
    pub total: u64,
    pub online: u64,
    pub offline: u64,
    pub errors: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceInvitation {
    pub invitation_code: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SpaceState {
    pub has_completed: bool,
    pub space_id: Option<String>,
    pub current_invitation: Option<SpaceInvitation>,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct Device {
    pub device_id: String,
    pub display_name: String,
    pub is_local: bool,
    pub online: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MemberRevocationOutcome {
    LocalOnly,
    Applied,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MemberRevocationResult {
    pub revocation_id: Option<String>,
    pub outcome: MemberRevocationOutcome,
    pub pending_recipients: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EntryNotResendableReason {
    RemoteOrigin,
    PayloadLost,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum ResendEntryOutcome {
    Completed {
        accepted: u64,
        duplicate: u64,
        offline: u64,
        errored: u64,
        pending: u64,
    },
    EntryNotFound {
        entry_id: String,
    },
    EntryNotResendable {
        entry_id: String,
        reason: EntryNotResendableReason,
    },
    TargetNotTrusted {
        device_id: String,
    },
    NoEligibleTargets,
}

enum WorkerCommand {
    RecoverSession {
        allow_secure_storage_unlock: bool,
        response: mpsc::Sender<Result<SessionRecovery, BindingError>>,
    },
    QueryLocalDevice {
        response: mpsc::Sender<Result<LocalDevice, BindingError>>,
    },
    RefreshPeerConnections {
        response: mpsc::Sender<Result<PeerConnectionRefresh, BindingError>>,
    },
    QuerySpaceState {
        response: mpsc::Sender<Result<SpaceState, BindingError>>,
    },
    ListDevices {
        response: mpsc::Sender<Result<Vec<Device>, BindingError>>,
    },
    RemoveMember {
        device_id: String,
        response: mpsc::Sender<Result<MemberRevocationResult, BindingError>>,
    },
    QueryMemberRevocation {
        revocation_id: String,
        response: mpsc::Sender<Result<Option<MemberRevocationResult>, BindingError>>,
    },
    ResendEntry {
        entry_id: String,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<ResendEntryOutcome, BindingError>>,
    },
    LeaveSpace {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    LifecycleState {
        response: mpsc::Sender<BindingEngineState>,
    },
    CreateSpace {
        device_name: Option<String>,
        passphrase: Zeroizing<String>,
        response: mpsc::Sender<Result<SpaceCreated, BindingError>>,
    },
    IssueInvitation {
        response: mpsc::Sender<Result<InvitationIssued, BindingError>>,
    },
    JoinSpace {
        invitation_code: Zeroizing<String>,
        device_name: Option<String>,
        passphrase: Zeroizing<String>,
        response: mpsc::Sender<Result<SpaceJoined, BindingError>>,
    },
    SendText {
        text: Zeroizing<String>,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<SendReport, BindingError>>,
    },
    SendImage {
        bytes: Zeroizing<Vec<u8>>,
        mime_type: String,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<SendReport, BindingError>>,
    },
    SendFiles {
        file_handles: Zeroizing<Vec<String>>,
        target_devices: Vec<String>,
        response: mpsc::Sender<Result<SendReport, BindingError>>,
    },
    CaptureCurrentClipboard {
        response: mpsc::Sender<Result<Option<String>, BindingError>>,
    },
    ObserveClipboardChange {
        dispatch: bool,
        response: mpsc::Sender<Result<Option<SendReport>, BindingError>>,
    },
    RestoreClipboard {
        entry_id: String,
        mode: BindingClipboardRestoreMode,
        response: mpsc::Sender<Result<BindingClipboardRestoreOutcome, BindingError>>,
    },
    QueryActiveClipboard {
        response: mpsc::Sender<Result<Option<ActiveClipboard>, BindingError>>,
    },
    ExportEntry {
        entry_id: String,
        destination_handle: Zeroizing<String>,
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    Suspend {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    Resume {
        response: mpsc::Sender<Result<(), BindingError>>,
    },
    Shutdown {
        deadline: Duration,
        response: mpsc::Sender<Result<(), BindingError>>,
    },
}

#[derive(uniffi::Object)]
pub struct MobileEngine {
    commands: Mutex<Option<tokio::sync::mpsc::UnboundedSender<WorkerCommand>>>,
    events: Arc<EventQueue>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

struct EventQueueState {
    events: VecDeque<BindingEvent>,
    lagged: bool,
    closed: bool,
}

struct EventQueue {
    capacity: usize,
    state: Mutex<EventQueueState>,
    ready: Condvar,
}

impl EventQueue {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(EventQueueState {
                events: VecDeque::new(),
                lagged: false,
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn push(&self, event: BindingEvent) {
        let mut state = lock(&self.state);
        if state.closed {
            return;
        }
        if state.events.len() == self.capacity {
            state.events.pop_front();
            state.lagged = true;
        }
        state.events.push_back(event);
        self.ready.notify_one();
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.ready.notify_all();
    }

    fn next(&self, timeout: Duration) -> Option<BindingEvent> {
        let deadline = Instant::now().checked_add(timeout);
        let mut state = lock(&self.state);
        loop {
            if state.lagged {
                state.lagged = false;
                return Some(BindingEvent::RefreshRequired {
                    reason: BindingRefreshReason::ConsumerLagged,
                });
            }
            if let Some(event) = state.events.pop_front() {
                return Some(event);
            }
            if state.closed || timeout.is_zero() {
                return None;
            }
            let remaining = deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_secs(60));
            if remaining.is_zero() {
                return None;
            }
            state = match self.ready.wait_timeout(state, remaining) {
                Ok((state, _)) => state,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
}

#[uniffi::export]
impl MobileEngine {
    #[uniffi::constructor]
    pub fn start(
        config: BindingConfig,
        host: Arc<dyn BindingHost>,
    ) -> Result<Arc<Self>, BindingError> {
        let capabilities = host_capabilities(Arc::clone(&host))?;
        let config = EngineConfig::new(config.app_version).with_profile_id(config.profile_id);
        let (commands, requests) = tokio::sync::mpsc::unbounded_channel();
        let events = Arc::new(EventQueue::new(256));
        let (started, start_result) = mpsc::channel();
        let worker_events = Arc::clone(&events);
        let worker = std::thread::Builder::new()
            .name("uc-engine-uniffi".to_owned())
            .spawn(move || run_worker(config, capabilities, requests, worker_events, started))
            .map_err(|_| BindingError::RuntimeUnavailable)?;

        match start_result.recv() {
            Ok(Ok(())) => Ok(Arc::new(Self {
                commands: Mutex::new(Some(commands)),
                events,
                worker: Mutex::new(Some(worker)),
            })),
            Ok(Err(error)) => {
                let _ = worker.join();
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                Err(BindingError::RuntimeUnavailable)
            }
        }
    }

    pub fn recover_session(
        &self,
        allow_secure_storage_unlock: bool,
    ) -> Result<SessionRecovery, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::RecoverSession {
                allow_secure_storage_unlock,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn query_local_device(&self) -> Result<LocalDevice, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::QueryLocalDevice { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn refresh_peer_connections(&self) -> Result<PeerConnectionRefresh, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::RefreshPeerConnections { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn query_space_state(&self) -> Result<SpaceState, BindingError> {
        self.request(|response| WorkerCommand::QuerySpaceState { response })
    }

    pub fn list_devices(&self) -> Result<Vec<Device>, BindingError> {
        self.request(|response| WorkerCommand::ListDevices { response })
    }

    pub fn remove_member(&self, device_id: String) -> Result<MemberRevocationResult, BindingError> {
        self.request(|response| WorkerCommand::RemoveMember {
            device_id,
            response,
        })
    }

    pub fn query_member_revocation(
        &self,
        revocation_id: String,
    ) -> Result<Option<MemberRevocationResult>, BindingError> {
        self.request(|response| WorkerCommand::QueryMemberRevocation {
            revocation_id,
            response,
        })
    }

    pub fn resend_entry(
        &self,
        entry_id: String,
        target_devices: Vec<String>,
    ) -> Result<ResendEntryOutcome, BindingError> {
        self.request(|response| WorkerCommand::ResendEntry {
            entry_id,
            target_devices,
            response,
        })
    }

    pub fn leave_space(&self) -> Result<(), BindingError> {
        self.request(|response| WorkerCommand::LeaveSpace { response })
    }

    pub fn lifecycle_state(&self) -> Result<BindingEngineState, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::LifecycleState { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result.recv().map_err(|_| BindingError::RuntimeUnavailable)
    }

    pub fn create_space(
        &self,
        device_name: Option<String>,
        passphrase: String,
    ) -> Result<SpaceCreated, BindingError> {
        let passphrase = Zeroizing::new(passphrase);
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::CreateSpace {
                device_name,
                passphrase,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn issue_invitation(&self) -> Result<InvitationIssued, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::IssueInvitation { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn join_space(
        &self,
        invitation_code: String,
        device_name: Option<String>,
        passphrase: String,
    ) -> Result<SpaceJoined, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::JoinSpace {
                invitation_code: Zeroizing::new(invitation_code),
                device_name,
                passphrase: Zeroizing::new(passphrase),
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn send_text(
        &self,
        text: String,
        target_devices: Vec<String>,
    ) -> Result<SendReport, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::SendText {
                text: Zeroizing::new(text),
                target_devices,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn send_image(
        &self,
        bytes: Vec<u8>,
        mime_type: String,
        target_devices: Vec<String>,
    ) -> Result<SendReport, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::SendImage {
                bytes: Zeroizing::new(bytes),
                mime_type,
                target_devices,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn send_files(
        &self,
        file_handles: Vec<String>,
        target_devices: Vec<String>,
    ) -> Result<SendReport, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::SendFiles {
                file_handles: Zeroizing::new(file_handles),
                target_devices,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn capture_current_clipboard(&self) -> Result<Option<String>, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::CaptureCurrentClipboard { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn observe_clipboard_change(
        &self,
        dispatch: bool,
    ) -> Result<Option<SendReport>, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::ObserveClipboardChange { dispatch, response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn restore_clipboard(
        &self,
        entry_id: String,
        mode: BindingClipboardRestoreMode,
    ) -> Result<BindingClipboardRestoreOutcome, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::RestoreClipboard {
                entry_id,
                mode,
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn query_active_clipboard(&self) -> Result<Option<ActiveClipboard>, BindingError> {
        self.request(|response| WorkerCommand::QueryActiveClipboard { response })
    }

    pub fn export_entry(
        &self,
        entry_id: String,
        destination_handle: String,
    ) -> Result<(), BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::ExportEntry {
                entry_id,
                destination_handle: Zeroizing::new(destination_handle),
                response,
            })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn shutdown(&self, deadline_ms: u64) -> Result<(), BindingError> {
        self.shutdown_inner(Duration::from_millis(deadline_ms), true)
    }

    pub fn suspend(&self) -> Result<(), BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::Suspend { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn resume(&self) -> Result<(), BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(WorkerCommand::Resume { response })
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    pub fn next_event(&self, timeout_ms: u64) -> Option<BindingEvent> {
        self.events.next(Duration::from_millis(timeout_ms))
    }
}

impl MobileEngine {
    fn request<T>(
        &self,
        command: impl FnOnce(mpsc::Sender<Result<T, BindingError>>) -> WorkerCommand,
    ) -> Result<T, BindingError> {
        let commands = self.command_sender()?;
        let (response, result) = mpsc::channel();
        commands
            .send(command(response))
            .map_err(|_| BindingError::RuntimeUnavailable)?;
        result
            .recv()
            .map_err(|_| BindingError::RuntimeUnavailable)?
    }

    fn command_sender(
        &self,
    ) -> Result<tokio::sync::mpsc::UnboundedSender<WorkerCommand>, BindingError> {
        lock(&self.commands)
            .as_ref()
            .cloned()
            .ok_or(BindingError::AlreadyStopped)
    }

    fn shutdown_inner(&self, deadline: Duration, join: bool) -> Result<(), BindingError> {
        let commands = lock(&self.commands)
            .take()
            .ok_or(BindingError::AlreadyStopped)?;
        let (response, result) = mpsc::channel();
        let shutdown_result = commands
            .send(WorkerCommand::Shutdown { deadline, response })
            .map_err(|_| BindingError::RuntimeUnavailable)
            .and_then(|()| {
                result
                    .recv()
                    .map_err(|_| BindingError::RuntimeUnavailable)?
            });
        let join_result = if join { self.join_worker() } else { Ok(()) };
        shutdown_result.and(join_result)
    }

    fn join_worker(&self) -> Result<(), BindingError> {
        if let Some(worker) = lock(&self.worker).take() {
            worker
                .join()
                .map_err(|_| BindingError::RuntimeUnavailable)?;
        }
        Ok(())
    }
}

impl Drop for MobileEngine {
    fn drop(&mut self) {
        let _ = self.shutdown_inner(Duration::ZERO, true);
    }
}

fn run_worker(
    config: EngineConfig,
    host: HostCapabilities,
    requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    events: Arc<EventQueue>,
    started: mpsc::Sender<Result<(), BindingError>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => {
            let _ = started.send(Err(BindingError::RuntimeUnavailable));
            return;
        }
    };
    runtime.block_on(run_worker_loop(config, host, requests, events, started));
}

async fn run_worker_loop(
    config: EngineConfig,
    host: HostCapabilities,
    mut requests: tokio::sync::mpsc::UnboundedReceiver<WorkerCommand>,
    events: Arc<EventQueue>,
    started: mpsc::Sender<Result<(), BindingError>>,
) {
    let (engine, mut engine_events) = match Engine::start(config, host).await {
        Ok(started_engine) => started_engine,
        Err(error) => {
            let _ = started.send(Err(error.into()));
            return;
        }
    };
    if started.send(Ok(())).is_err() {
        let _ = engine.shutdown(Duration::ZERO).await;
        return;
    }

    let event_task = tokio::spawn(async move {
        while let Some(event) = engine_events.next().await {
            events.push(map_engine_event(event));
        }
        events.close();
    });

    let mut shutdown_response = None;

    while let Some(command) = requests.recv().await {
        match command {
            WorkerCommand::RecoverSession {
                allow_secure_storage_unlock,
                response,
            } => {
                let result = engine
                    .execute(Operation::RecoverSession(RecoverSessionInput {
                        allow_secure_storage_unlock,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_session_recovery);
                let _ = response.send(result);
            }
            WorkerCommand::QueryLocalDevice { response } => {
                let result = engine
                    .execute(Operation::QueryLocalDevice)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_local_device);
                let _ = response.send(result);
            }
            WorkerCommand::RefreshPeerConnections { response } => {
                let result = engine
                    .execute(Operation::RefreshPeerConnections)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_peer_connection_refresh);
                let _ = response.send(result);
            }
            WorkerCommand::QuerySpaceState { response } => {
                let result = engine
                    .execute(Operation::QuerySetupState)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_space_state);
                let _ = response.send(result);
            }
            WorkerCommand::ListDevices { response } => {
                let result = engine
                    .execute(Operation::ListDevices)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_devices);
                let _ = response.send(result);
            }
            WorkerCommand::RemoveMember {
                device_id,
                response,
            } => {
                let result = engine
                    .execute(Operation::RemoveMember(RemoveMemberInput { device_id }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_member_removed);
                let _ = response.send(result);
            }
            WorkerCommand::QueryMemberRevocation {
                revocation_id,
                response,
            } => {
                let result = engine
                    .execute(Operation::QueryMemberRevocation(
                        QueryMemberRevocationInput { revocation_id },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_member_revocation_status);
                let _ = response.send(result);
            }
            WorkerCommand::ResendEntry {
                entry_id,
                target_devices,
                response,
            } => {
                let result = engine
                    .execute(Operation::ResendEntry(ResendEntryInput {
                        entry_id,
                        target_devices,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_resend_outcome);
                let _ = response.send(result);
            }
            WorkerCommand::LeaveSpace { response } => {
                let result = engine
                    .execute(Operation::FactoryResetSpace)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_space_left);
                let _ = response.send(result);
            }
            WorkerCommand::LifecycleState { response } => {
                let _ = response.send(map_engine_state(engine.lifecycle_state().await));
            }
            WorkerCommand::CreateSpace {
                device_name,
                passphrase,
                response,
            } => {
                let result = engine
                    .execute(Operation::CreateSpace(CreateSpaceInput {
                        device_name,
                        passphrase: SecretString::new(passphrase.as_str()),
                        passphrase_confirmation: SecretString::new(passphrase.as_str()),
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_space_created);
                let _ = response.send(result);
            }
            WorkerCommand::IssueInvitation { response } => {
                let result = engine
                    .execute(Operation::IssueInvitation)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_invitation_issued);
                let _ = response.send(result);
            }
            WorkerCommand::JoinSpace {
                mut invitation_code,
                device_name,
                passphrase,
                response,
            } => {
                let result = engine
                    .execute(Operation::JoinSpace(JoinSpaceInput {
                        invitation_code: std::mem::take(&mut *invitation_code),
                        device_name,
                        passphrase: SecretString::new(passphrase.as_str()),
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_space_joined);
                let _ = response.send(result);
            }
            WorkerCommand::SendText {
                mut text,
                target_devices,
                response,
            } => {
                let result = engine
                    .execute(Operation::SendText(SendTextInput {
                        text: std::mem::take(&mut *text),
                        target_devices,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_send_report);
                let _ = response.send(result);
            }
            WorkerCommand::SendImage {
                mut bytes,
                mime_type,
                target_devices,
                response,
            } => {
                let result = engine
                    .execute(Operation::SendImage(SendImageInput {
                        bytes: std::mem::take(&mut *bytes),
                        mime_type,
                        target_devices,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_send_report);
                let _ = response.send(result);
            }
            WorkerCommand::SendFiles {
                file_handles,
                target_devices,
                response,
            } => {
                let result = engine
                    .execute(Operation::SendFiles(SendFilesInput {
                        files: file_handles
                            .iter()
                            .map(|handle| HostFileHandle::new(handle.to_owned()))
                            .collect(),
                        target_devices,
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_send_report);
                let _ = response.send(result);
            }
            WorkerCommand::CaptureCurrentClipboard { response } => {
                let result = engine
                    .execute(Operation::CaptureCurrentClipboard)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_clipboard_captured);
                let _ = response.send(result);
            }
            WorkerCommand::ObserveClipboardChange { dispatch, response } => {
                let result = engine
                    .execute(Operation::ObserveClipboardChange(
                        ObserveClipboardChangeInput { dispatch },
                    ))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_clipboard_change_observed);
                let _ = response.send(result);
            }
            WorkerCommand::RestoreClipboard {
                entry_id,
                mode,
                response,
            } => {
                let result = engine
                    .execute(Operation::RestoreClipboard(RestoreClipboardInput {
                        entry_id,
                        mode: map_restore_mode(mode),
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_clipboard_restored);
                let _ = response.send(result);
            }
            WorkerCommand::QueryActiveClipboard { response } => {
                let result = engine
                    .execute(Operation::QueryActiveClipboard)
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_active_clipboard);
                let _ = response.send(result);
            }
            WorkerCommand::ExportEntry {
                entry_id,
                destination_handle,
                response,
            } => {
                let result = engine
                    .execute(Operation::ExportEntry(ExportEntryInput {
                        entry_id,
                        destination: HostFileHandle::new(destination_handle.as_str()),
                    }))
                    .await
                    .map_err(BindingError::from)
                    .and_then(map_entry_exported);
                let _ = response.send(result);
            }
            WorkerCommand::Suspend { response } => {
                let result = engine.suspend().await.map_err(BindingError::from);
                let _ = response.send(result);
            }
            WorkerCommand::Resume { response } => {
                let result = engine.resume().await.map_err(BindingError::from);
                let _ = response.send(result);
            }
            WorkerCommand::Shutdown { deadline, response } => {
                let result = engine.shutdown(deadline).await.map_err(BindingError::from);
                shutdown_response = Some((response, result));
                break;
            }
        }
    }
    if shutdown_response.is_none() {
        let _ = engine.shutdown(Duration::ZERO).await;
    }
    let _ = event_task.await;
    if let Some((response, result)) = shutdown_response {
        let _ = response.send(result);
    }
}

fn map_engine_event(event: uc_engine::EngineEvent) -> BindingEvent {
    match event {
        uc_engine::EngineEvent::StateChanged { state } => BindingEvent::StateChanged {
            state: map_engine_state(state),
        },
        uc_engine::EngineEvent::OperationFinished {
            operation_id,
            terminal,
        } => {
            let (terminal, failure) = match terminal {
                uc_engine::OperationTerminal::Succeeded => {
                    (BindingOperationTerminal::Succeeded, None)
                }
                uc_engine::OperationTerminal::Failed(error) => (
                    BindingOperationTerminal::Failed,
                    Some(BindingFailure::from(error)),
                ),
                uc_engine::OperationTerminal::Cancelled => {
                    (BindingOperationTerminal::Cancelled, None)
                }
            };
            BindingEvent::OperationFinished {
                operation_id,
                terminal,
                failure,
            }
        }
        uc_engine::EngineEvent::LifecycleFailed { action, error } => {
            BindingEvent::LifecycleFailed {
                action: match action {
                    uc_engine::LifecycleAction::Suspend => BindingLifecycleAction::Suspend,
                    uc_engine::LifecycleAction::Resume => BindingLifecycleAction::Resume,
                },
                failure: BindingFailure::from(error),
            }
        }
        uc_engine::EngineEvent::RefreshRequired { reason } => BindingEvent::RefreshRequired {
            reason: match reason {
                uc_engine::RefreshReason::ConsumerLagged => BindingRefreshReason::ConsumerLagged,
                uc_engine::RefreshReason::StateInvalidated => {
                    BindingRefreshReason::StateInvalidated
                }
            },
        },
        uc_engine::EngineEvent::Fatal { error } => BindingEvent::Fatal {
            failure: BindingFailure::from(error),
        },
        uc_engine::EngineEvent::IncomingEntry(event) => BindingEvent::IncomingEntry {
            entry_id: event.entry_id,
            attempt_id: event.attempt_id,
            preview: event.preview,
            origin: match event.origin {
                uc_engine::ClipboardOriginSummary::Local => BindingClipboardOrigin::Local,
                uc_engine::ClipboardOriginSummary::Remote => BindingClipboardOrigin::Remote,
            },
        },
        uc_engine::EngineEvent::IncomingPending(event) => BindingEvent::IncomingPending {
            entry_id: event.entry_id,
            attempt_id: event.attempt_id,
            from_device: event.from_device,
            total_bytes: event.total_bytes,
            filenames: event.filenames,
        },
        uc_engine::EngineEvent::ReceiveAttemptStateChanged(event) => {
            BindingEvent::ReceiveAttemptStateChanged {
                entry_id: event.entry_id,
                attempt_id: event.attempt_id,
                state: event.state,
            }
        }
        uc_engine::EngineEvent::DeliveryStatusChanged(event) => {
            BindingEvent::DeliveryStatusChanged {
                entry_id: event.entry_id,
                target_device_id: event.target_device_id,
            }
        }
        uc_engine::EngineEvent::PeerPresenceChanged(event) => BindingEvent::PeerPresenceChanged {
            device_id: event.device_id,
            state: event.state,
            at_ms: event.at_ms,
        },
        uc_engine::EngineEvent::TransferProgress(event) => BindingEvent::TransferProgress {
            transfer_id: event.transfer_id,
            entry_id: event.entry_id,
            attempt_id: event.attempt_id,
            peer_id: event.peer_id,
            direction: match event.direction {
                uc_engine::TransferDirectionSummary::Sending => BindingTransferDirection::Sending,
                uc_engine::TransferDirectionSummary::Receiving => {
                    BindingTransferDirection::Receiving
                }
            },
            completed_bytes: event.completed_bytes,
            total_bytes: event.total_bytes,
        },
        uc_engine::EngineEvent::TransferStatusChanged(event) => {
            BindingEvent::TransferStatusChanged {
                transfer_id: event.transfer_id,
                entry_id: event.entry_id,
                attempt_id: event.attempt_id,
                status: event.status,
                reason: event.reason,
            }
        }
        uc_engine::EngineEvent::ActiveClipboardChanged(event) => {
            BindingEvent::ActiveClipboardChanged {
                snapshot_hash: event.snapshot_hash,
                entry_id: event.entry_id,
                activated_at_ms: event.activated_at_ms,
                activated_by: event.activated_by,
            }
        }
        other => BindingEvent::Changed {
            kind: other.kind().to_owned(),
        },
    }
}

fn map_engine_state(state: uc_engine::EngineState) -> BindingEngineState {
    match state {
        uc_engine::EngineState::Running => BindingEngineState::Running,
        uc_engine::EngineState::Quiescing => BindingEngineState::Quiescing,
        uc_engine::EngineState::Quiesced => BindingEngineState::Quiesced,
        uc_engine::EngineState::Suspended => BindingEngineState::Suspended,
        uc_engine::EngineState::ShuttingDown => BindingEngineState::ShuttingDown,
        uc_engine::EngineState::Stopped => BindingEngineState::Stopped,
    }
}

fn map_space_created(result: OperationResult) -> Result<SpaceCreated, BindingError> {
    match result {
        OperationResult::SpaceCreated {
            space_id,
            self_device_id,
            identity_fingerprint,
        } => Ok(SpaceCreated {
            space_id,
            self_device_id,
            identity_fingerprint,
        }),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_space_state(result: OperationResult) -> Result<SpaceState, BindingError> {
    match result {
        OperationResult::SetupState(state) => Ok(SpaceState {
            has_completed: state.has_completed,
            space_id: state.space_id,
            current_invitation: state.current_invitation.map(|invitation| SpaceInvitation {
                invitation_code: invitation.invitation_code,
                expires_at_ms: invitation.expires_at_ms,
            }),
            device_name: state.device_name,
        }),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_devices(result: OperationResult) -> Result<Vec<Device>, BindingError> {
    match result {
        OperationResult::Devices(devices) => Ok(devices
            .into_iter()
            .map(|device| Device {
                device_id: device.device_id,
                display_name: device.display_name,
                is_local: device.is_local,
                online: device.online,
            })
            .collect()),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_member_removed(result: OperationResult) -> Result<MemberRevocationResult, BindingError> {
    match result {
        OperationResult::MemberRemoved(summary) => Ok(map_member_revocation_summary(summary)),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_member_revocation_status(
    result: OperationResult,
) -> Result<Option<MemberRevocationResult>, BindingError> {
    match result {
        OperationResult::MemberRevocationStatus(summary) => {
            Ok(summary.map(map_member_revocation_summary))
        }
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_member_revocation_summary(
    summary: uc_engine::MemberRevocationSummary,
) -> MemberRevocationResult {
    MemberRevocationResult {
        revocation_id: summary.revocation_id,
        outcome: match summary.outcome {
            uc_engine::MemberRevocationOutcome::LocalOnly => MemberRevocationOutcome::LocalOnly,
            uc_engine::MemberRevocationOutcome::Applied => MemberRevocationOutcome::Applied,
            uc_engine::MemberRevocationOutcome::Complete => MemberRevocationOutcome::Complete,
        },
        pending_recipients: summary.pending_recipients,
    }
}

fn map_resend_outcome(result: OperationResult) -> Result<ResendEntryOutcome, BindingError> {
    match result {
        OperationResult::EntryResent(outcome) => match outcome {
            uc_engine::ResendEntryOutcome::Completed(report) => Ok(ResendEntryOutcome::Completed {
                accepted: count_to_u64(report.accepted)?,
                duplicate: count_to_u64(report.duplicate)?,
                offline: count_to_u64(report.offline)?,
                errored: count_to_u64(report.errored)?,
                pending: count_to_u64(report.pending)?,
            }),
            uc_engine::ResendEntryOutcome::EntryNotFound { entry_id } => {
                Ok(ResendEntryOutcome::EntryNotFound { entry_id })
            }
            uc_engine::ResendEntryOutcome::EntryNotResendable { entry_id, reason } => {
                Ok(ResendEntryOutcome::EntryNotResendable {
                    entry_id,
                    reason: match reason {
                        uc_engine::EntryNotResendableReason::RemoteOrigin => {
                            EntryNotResendableReason::RemoteOrigin
                        }
                        uc_engine::EntryNotResendableReason::PayloadLost => {
                            EntryNotResendableReason::PayloadLost
                        }
                    },
                })
            }
            uc_engine::ResendEntryOutcome::TargetNotTrusted { device_id } => {
                Ok(ResendEntryOutcome::TargetNotTrusted { device_id })
            }
            uc_engine::ResendEntryOutcome::NoEligibleTargets => {
                Ok(ResendEntryOutcome::NoEligibleTargets)
            }
        },
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_space_left(result: OperationResult) -> Result<(), BindingError> {
    match result {
        OperationResult::SpaceFactoryReset => Ok(()),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_session_recovery(result: OperationResult) -> Result<SessionRecovery, BindingError> {
    match result {
        OperationResult::SessionRecovered { unlocked, resumed } => {
            Ok(SessionRecovery { unlocked, resumed })
        }
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_local_device(result: OperationResult) -> Result<LocalDevice, BindingError> {
    match result {
        OperationResult::LocalDevice(device) => Ok(LocalDevice {
            device_id: device.device_id,
            display_name: device.display_name,
        }),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_invitation_issued(result: OperationResult) -> Result<InvitationIssued, BindingError> {
    match result {
        OperationResult::InvitationIssued {
            invitation_code,
            expires_at_ms,
            availability,
        } => Ok(InvitationIssued {
            invitation_code,
            expires_at_ms,
            availability: match availability {
                uc_engine::InvitationAvailability::CrossNetwork => {
                    InvitationAvailability::CrossNetwork
                }
                uc_engine::InvitationAvailability::SameLocalNetwork => {
                    InvitationAvailability::SameLocalNetwork
                }
            },
        }),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_space_joined(result: OperationResult) -> Result<SpaceJoined, BindingError> {
    match result {
        OperationResult::SpaceJoined {
            sponsor_device_id,
            sponsor_identity_fingerprint,
            space_id,
            self_device_id,
            self_identity_fingerprint,
            migrated_records,
        } => Ok(SpaceJoined {
            sponsor_device_id,
            sponsor_identity_fingerprint,
            space_id,
            self_device_id,
            self_identity_fingerprint,
            migrated_records,
        }),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_send_report(result: OperationResult) -> Result<SendReport, BindingError> {
    match result {
        OperationResult::EntrySent(report) => Ok(SendReport {
            entry_id: report.entry_id,
            at_ms: report.at_ms,
            total_accepted: count_to_u64(report.total_accepted)?,
            total_duplicate: count_to_u64(report.total_duplicate)?,
            total_offline: count_to_u64(report.total_offline)?,
            total_errored: count_to_u64(report.total_errored)?,
            total_pending: count_to_u64(report.total_pending)?,
        }),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_peer_connection_refresh(
    result: OperationResult,
) -> Result<PeerConnectionRefresh, BindingError> {
    match result {
        OperationResult::PeerConnectionsRefreshed(report) => Ok(PeerConnectionRefresh {
            total: u64::from(report.total),
            online: u64::from(report.online),
            offline: u64::from(report.offline),
            errors: u64::from(report.errors),
        }),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_clipboard_change_observed(
    result: OperationResult,
) -> Result<Option<SendReport>, BindingError> {
    match result {
        OperationResult::ClipboardChangeObserved { report } => report
            .map(|report| map_send_report(OperationResult::EntrySent(report)))
            .transpose(),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_clipboard_captured(result: OperationResult) -> Result<Option<String>, BindingError> {
    match result {
        OperationResult::ClipboardCaptured { entry_id } => Ok(entry_id),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_restore_mode(mode: BindingClipboardRestoreMode) -> ClipboardRestoreMode {
    match mode {
        BindingClipboardRestoreMode::Standard => ClipboardRestoreMode::Standard,
        BindingClipboardRestoreMode::PlainText => ClipboardRestoreMode::PlainText,
        BindingClipboardRestoreMode::FilePaths => ClipboardRestoreMode::FilePaths,
    }
}

fn map_clipboard_restored(
    result: OperationResult,
) -> Result<BindingClipboardRestoreOutcome, BindingError> {
    match result {
        OperationResult::ClipboardRestored(ClipboardRestoreOutcome::Restored) => {
            Ok(BindingClipboardRestoreOutcome::Restored)
        }
        OperationResult::ClipboardRestored(ClipboardRestoreOutcome::PayloadUnavailable {
            ..
        }) => Ok(BindingClipboardRestoreOutcome::PayloadUnavailable),
        OperationResult::ClipboardRestored(ClipboardRestoreOutcome::NotApplicable { .. }) => {
            Ok(BindingClipboardRestoreOutcome::NotApplicable)
        }
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_active_clipboard(result: OperationResult) -> Result<Option<ActiveClipboard>, BindingError> {
    match result {
        OperationResult::ActiveClipboard(active) => Ok(active.map(|active| ActiveClipboard {
            entry_id: active.entry_id,
            activated_by: active.activated_by,
        })),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn map_entry_exported(result: OperationResult) -> Result<(), BindingError> {
    match result {
        OperationResult::EntryExported => Ok(()),
        _ => Err(BindingError::UnexpectedResult),
    }
}

fn count_to_u64(value: usize) -> Result<u64, BindingError> {
    u64::try_from(value).map_err(|_| BindingError::UnexpectedResult)
}

fn host_capabilities(host: Arc<dyn BindingHost>) -> Result<HostCapabilities, BindingError> {
    let directories = HostDirectories::new(
        host_path(host.private_data_directory())?,
        host_path(host.cache_directory())?,
        host_path(host.temporary_directory())?,
    );
    for directory in [
        directories.private_data(),
        directories.cache(),
        directories.temporary(),
    ] {
        std::fs::create_dir_all(directory).map_err(|_| BindingError::HostIo)?;
    }
    Ok(HostCapabilities::new(
        directories,
        Box::new(BindingSecureStorage {
            host: Arc::clone(&host),
        }),
        Box::new(BindingClipboard {
            host: Arc::clone(&host),
        }),
        Box::new(BindingFiles { host }),
    ))
}

fn host_path(result: Result<String, HostBindingError>) -> Result<PathBuf, BindingError> {
    result.map(PathBuf::from).map_err(map_binding_host_error)
}

fn map_binding_host_error(error: HostBindingError) -> BindingError {
    match error {
        HostBindingError::Unavailable => BindingError::HostUnavailable,
        HostBindingError::PermissionDenied => BindingError::HostPermissionDenied,
        HostBindingError::InvalidHandle => BindingError::HostInvalidHandle,
        HostBindingError::Io => BindingError::HostIo,
    }
}

fn map_host_capability_error(error: HostBindingError) -> HostCapabilityError {
    let category = match error {
        HostBindingError::Unavailable => HostCapabilityErrorCategory::Unavailable,
        HostBindingError::PermissionDenied => HostCapabilityErrorCategory::PermissionDenied,
        HostBindingError::InvalidHandle => HostCapabilityErrorCategory::InvalidHandle,
        HostBindingError::Io => HostCapabilityErrorCategory::Io,
    };
    HostCapabilityError::new(category, "binding host callback failed")
}

struct BindingSecureStorage {
    host: Arc<dyn BindingHost>,
}

impl HostSecureStorage for BindingSecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        self.host
            .secure_storage_get(key.to_owned())
            .map_err(map_host_capability_error)
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.host
            .secure_storage_set(key.to_owned(), value.to_vec())
            .map_err(map_host_capability_error)
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.host
            .secure_storage_delete(key.to_owned())
            .map_err(map_host_capability_error)
    }
}

struct BindingClipboard {
    host: Arc<dyn BindingHost>,
}

impl HostClipboard for BindingClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        self.host
            .clipboard_read()
            .map(|snapshot| HostClipboardSnapshot {
                observed_at_ms: snapshot.observed_at_ms,
                representations: snapshot
                    .representations
                    .into_iter()
                    .map(map_clipboard_representation)
                    .collect(),
            })
            .map_err(map_host_capability_error)
    }

    fn write(&self, snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        self.host
            .clipboard_write(BindingClipboardSnapshot {
                observed_at_ms: snapshot.observed_at_ms,
                representations: snapshot
                    .representations
                    .into_iter()
                    .map(map_engine_clipboard_representation)
                    .collect(),
            })
            .map_err(map_host_capability_error)
    }
}

fn map_clipboard_representation(
    representation: BindingClipboardRepresentation,
) -> HostClipboardRepresentation {
    match representation {
        BindingClipboardRepresentation::Inline {
            format,
            mime_type,
            bytes,
        } => HostClipboardRepresentation::Inline {
            format,
            mime_type,
            bytes,
        },
        BindingClipboardRepresentation::File {
            format,
            handle,
            display_name,
            mime_type,
            size_bytes,
        } => HostClipboardRepresentation::File {
            format,
            handle: HostFileHandle::new(handle),
            display_name,
            mime_type,
            size_bytes,
        },
    }
}

fn map_engine_clipboard_representation(
    representation: HostClipboardRepresentation,
) -> BindingClipboardRepresentation {
    match representation {
        HostClipboardRepresentation::Inline {
            format,
            mime_type,
            bytes,
        } => BindingClipboardRepresentation::Inline {
            format,
            mime_type,
            bytes,
        },
        HostClipboardRepresentation::File {
            format,
            handle,
            display_name,
            mime_type,
            size_bytes,
        } => BindingClipboardRepresentation::File {
            format,
            handle: handle.as_str().to_owned(),
            display_name,
            mime_type,
            size_bytes,
        },
    }
}

struct BindingFiles {
    host: Arc<dyn BindingHost>,
}

impl HostFileAccess for BindingFiles {
    fn metadata(&self, handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        self.host
            .file_metadata(handle.as_str().to_owned())
            .map(|metadata: BindingFileMetadata| HostFileMetadata {
                display_name: metadata.display_name,
                size_bytes: metadata.size_bytes,
                mime_type: metadata.mime_type,
            })
            .map_err(map_host_capability_error)
    }

    fn read_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        self.host
            .file_read_chunk(handle.as_str().to_owned(), offset, max_bytes)
            .map_err(map_host_capability_error)
    }

    fn write_chunk(
        &self,
        handle: &HostFileHandle,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        self.host
            .file_write_chunk(handle.as_str().to_owned(), offset, bytes.to_vec())
            .map_err(map_host_capability_error)
    }

    fn finish_write(&self, handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
        self.host
            .file_finish_write(handle.as_str().to_owned())
            .map_err(map_host_capability_error)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_event_queue_reports_lag_and_keeps_the_latest_events() {
        let queue = EventQueue::new(2);
        queue.push(BindingEvent::Changed {
            kind: "first".to_owned(),
        });
        queue.push(BindingEvent::Changed {
            kind: "second".to_owned(),
        });
        queue.push(BindingEvent::Changed {
            kind: "third".to_owned(),
        });

        assert_eq!(
            queue.next(Duration::ZERO),
            Some(BindingEvent::RefreshRequired {
                reason: BindingRefreshReason::ConsumerLagged,
            })
        );
        assert_eq!(
            queue.next(Duration::ZERO),
            Some(BindingEvent::Changed {
                kind: "second".to_owned(),
            })
        );
        assert_eq!(
            queue.next(Duration::ZERO),
            Some(BindingEvent::Changed {
                kind: "third".to_owned(),
            })
        );
        assert_eq!(queue.next(Duration::ZERO), None);
    }

    #[test]
    fn event_queue_handles_the_largest_foreign_timeout_without_panicking() {
        let queue = EventQueue::new(1);
        queue.close();

        assert_eq!(queue.next(Duration::MAX), None);
    }

    #[test]
    fn invitation_availability_survives_the_mobile_binding() {
        for (engine_availability, binding_availability) in [
            (
                uc_engine::InvitationAvailability::CrossNetwork,
                InvitationAvailability::CrossNetwork,
            ),
            (
                uc_engine::InvitationAvailability::SameLocalNetwork,
                InvitationAvailability::SameLocalNetwork,
            ),
        ] {
            let invitation = map_invitation_issued(OperationResult::InvitationIssued {
                invitation_code: "NEVER-SHOW".to_owned(),
                expires_at_ms: 1,
                availability: engine_availability,
            })
            .expect("invitation result must map into the mobile binding");

            assert_eq!(invitation.availability, binding_availability);
        }
    }

    #[test]
    fn lifecycle_failure_keeps_the_action_and_stable_failure() {
        let event = map_engine_event(uc_engine::EngineEvent::LifecycleFailed {
            action: uc_engine::LifecycleAction::Suspend,
            error: uc_engine::EngineError::new(
                1214,
                uc_engine::EngineErrorCategory::Unavailable,
                true,
            ),
        });

        assert_eq!(
            event,
            BindingEvent::LifecycleFailed {
                action: BindingLifecycleAction::Suspend,
                failure: BindingFailure {
                    code: 1214,
                    category: crate::BindingErrorCategory::Unavailable,
                    retryable: true,
                },
            }
        );
    }

    #[test]
    fn incoming_entry_event_keeps_history_identity_and_origin() {
        let event = map_engine_event(uc_engine::EngineEvent::IncomingEntry(
            uc_engine::IncomingEntryEvent {
                entry_id: "entry-1".to_owned(),
                attempt_id: Some("attempt-1".to_owned()),
                preview: "New clipboard content".to_owned(),
                origin: uc_engine::ClipboardOriginSummary::Remote,
            },
        ));

        assert_eq!(
            event,
            BindingEvent::IncomingEntry {
                entry_id: "entry-1".to_owned(),
                attempt_id: Some("attempt-1".to_owned()),
                preview: "New clipboard content".to_owned(),
                origin: BindingClipboardOrigin::Remote,
            }
        );
    }

    #[test]
    fn delivery_and_presence_events_keep_their_target_state() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::DeliveryStatusChanged(
                uc_engine::DeliveryStatusChanged {
                    entry_id: "entry-1".to_owned(),
                    target_device_id: "device-2".to_owned(),
                },
            )),
            BindingEvent::DeliveryStatusChanged {
                entry_id: "entry-1".to_owned(),
                target_device_id: "device-2".to_owned(),
            }
        );
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::PeerPresenceChanged(
                uc_engine::PeerPresenceChanged {
                    device_id: "device-2".to_owned(),
                    state: "online".to_owned(),
                    at_ms: 42,
                },
            )),
            BindingEvent::PeerPresenceChanged {
                device_id: "device-2".to_owned(),
                state: "online".to_owned(),
                at_ms: 42,
            }
        );
    }

    #[test]
    fn transfer_events_keep_progress_and_terminal_details() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::TransferProgress(
                uc_engine::TransferProgress {
                    transfer_id: "transfer-1".to_owned(),
                    entry_id: Some("entry-1".to_owned()),
                    attempt_id: Some("attempt-1".to_owned()),
                    peer_id: "device-2".to_owned(),
                    direction: uc_engine::TransferDirectionSummary::Receiving,
                    completed_bytes: 64,
                    total_bytes: Some(128),
                },
            )),
            BindingEvent::TransferProgress {
                transfer_id: "transfer-1".to_owned(),
                entry_id: Some("entry-1".to_owned()),
                attempt_id: Some("attempt-1".to_owned()),
                peer_id: "device-2".to_owned(),
                direction: BindingTransferDirection::Receiving,
                completed_bytes: 64,
                total_bytes: Some(128),
            }
        );
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::TransferStatusChanged(
                uc_engine::TransferStatusChanged {
                    transfer_id: "transfer-1".to_owned(),
                    entry_id: "entry-1".to_owned(),
                    attempt_id: Some("attempt-1".to_owned()),
                    status: "completed".to_owned(),
                    reason: None,
                },
            )),
            BindingEvent::TransferStatusChanged {
                transfer_id: "transfer-1".to_owned(),
                entry_id: "entry-1".to_owned(),
                attempt_id: Some("attempt-1".to_owned()),
                status: "completed".to_owned(),
                reason: None,
            }
        );
    }

    #[test]
    fn pending_receive_event_keeps_entry_and_file_summary() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::IncomingPending(
                uc_engine::IncomingPendingEvent {
                    entry_id: "entry-1".to_owned(),
                    attempt_id: Some("attempt-1".to_owned()),
                    from_device: "device-2".to_owned(),
                    total_bytes: Some(256),
                    filenames: vec!["private-name.txt".to_owned()],
                },
            )),
            BindingEvent::IncomingPending {
                entry_id: "entry-1".to_owned(),
                attempt_id: Some("attempt-1".to_owned()),
                from_device: "device-2".to_owned(),
                total_bytes: Some(256),
                filenames: vec!["private-name.txt".to_owned()],
            }
        );
    }

    #[test]
    fn receive_attempt_and_active_clipboard_events_keep_refresh_identity() {
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::ReceiveAttemptStateChanged(
                uc_engine::ReceiveAttemptStateChanged {
                    entry_id: "entry-1".to_owned(),
                    attempt_id: "attempt-1".to_owned(),
                    state: "completed".to_owned(),
                },
            )),
            BindingEvent::ReceiveAttemptStateChanged {
                entry_id: "entry-1".to_owned(),
                attempt_id: "attempt-1".to_owned(),
                state: "completed".to_owned(),
            }
        );
        assert_eq!(
            map_engine_event(uc_engine::EngineEvent::ActiveClipboardChanged(
                uc_engine::ActiveClipboardChanged {
                    snapshot_hash: "snapshot-1".to_owned(),
                    entry_id: "entry-1".to_owned(),
                    activated_at_ms: 42,
                    activated_by: "device-2".to_owned(),
                },
            )),
            BindingEvent::ActiveClipboardChanged {
                snapshot_hash: "snapshot-1".to_owned(),
                entry_id: "entry-1".to_owned(),
                activated_at_ms: 42,
                activated_by: "device-2".to_owned(),
            }
        );
    }
}
