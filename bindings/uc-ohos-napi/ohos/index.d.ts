export interface OhFileMetadata {
  displayName: string
  sizeBytes: string
  mimeType?: string
}

export interface OhClipboardRepresentation {
  kind: string
  format: string
  mimeType?: string
  bytes?: Uint8Array
  handle?: string
  displayName?: string
  sizeBytes?: string
}

export interface OhClipboardSnapshot {
  observedAtMs: number
  representations: OhClipboardRepresentation[]
}

export interface OhHost {
  privateDataDirectory: string
  cacheDirectory: string
  temporaryDirectory: string
  secureStorageGet(key: string): OhHostResult<Uint8Array | null>
  secureStorageSet(key: string, value: Uint8Array): OhHostResult<void>
  secureStorageDelete(key: string): OhHostResult<void>
  fileMetadata(handle: string): OhHostResult<OhFileMetadata>
  fileReadChunk(handle: string, offset: string, maxBytes: number): OhHostResult<Uint8Array>
  fileWriteChunk(handle: string, offset: string, bytes: Uint8Array): OhHostResult<void>
  fileFinishWrite(handle: string): OhHostResult<void>
  clipboardRead(): OhHostResult<OhClipboardSnapshot>
  clipboardWrite(snapshot: OhClipboardSnapshot): OhHostResult<void>
}

export interface OhHostResult<T> {
  ok: boolean
  value?: T
  errorCategory?: string
}

export type PreparedHost = object

export interface OhHostDirectories {
  privateDataDirectory: string
  cacheDirectory: string
  temporaryDirectory: string
}

export interface OhCollectorConfig {
  traceEndpoint: string
  logEndpoint: string
  authHeaderName?: string
  authHeaderValue?: string
}

export interface OhObservabilityConfig {
  serviceVersion: string
  environment: 'development' | 'test' | 'staging' | 'production'
  appChannel: string
  remoteDiagnosticsEnabled: boolean
  collector?: OhCollectorConfig
}

export interface OhObservabilitySetup {
  reused: boolean
  remote: 'disabled' | 'ready' | 'unavailable'
  localFile: 'disabled' | 'ready' | 'unavailable'
  droppedLocalRecords: number
}

export interface OhObservabilityHealth {
  remote: 'disabled' | 'ready' | 'unavailable'
  localFile: 'disabled' | 'ready' | 'unavailable'
  droppedLocalRecords: number
  droppedRemoteSpans: number
  droppedRemoteLogs: number
  failedRemoteSpanBatches: number
  failedRemoteLogBatches: number
}

export interface OhObservabilitySignalSummary {
  traces: 'completed' | 'failed' | 'timed_out' | 'already_shutdown'
  logs: 'completed' | 'failed' | 'timed_out' | 'already_shutdown'
}

export interface OhSendReport {
  entryId: string
  atMs: number
  totalAccepted: number
  totalDuplicate: number
  totalOffline: number
  totalErrored: number
  totalPending: number
}

export interface OhSessionRecovery {
  unlocked: boolean
  resumed: boolean
}

export interface OhNetworkRecoveryStatus {
  phase: 'idle' | 'recovering' | 'retry_scheduled' | 'failed'
  retryable: boolean
  nextRetryInMs?: number
}

export interface OhLocalDevice {
  deviceId: string
  displayName: string
}

export interface OhInvitationIssued {
  invitationCode: string
  fullInvitation: string
  expiresAtMs: number
  availability: string
}

export interface OhJoinedSpace {
  sponsorDeviceId: string
  sponsorIdentityFingerprint: string
  spaceId: string
  selfDeviceId: string
  selfIdentityFingerprint: string
  migratedRecords?: string
  preservedUnreadableRecords?: string
}

export interface OhJoinSpaceStatus {
  status: 'active' | 'pending' | 'rejected'
  joinId: string
  joinedSpace?: OhJoinedSpace
  targetSpaceId?: string
  sponsorDeviceId?: string
  sponsorIdentityFingerprint?: string
  cancelRequested?: boolean
  peerUpgradeRequired: boolean
  rejectionReason?: string
}

export interface OhMembershipConvergence {
  state: 'complete' | 'converging' | 'waiting_for_upgrade' | 'blocked'
  pendingCount: number
  waitingForPeerCount: number
  waitingForUpdateCount: number
  versionIncompatibleCount: number
  blockedCount: number
  rejectedCount: number
}

export interface OhSharedDeviceRefreshStarted {
  requestId: string
}

export interface OhSharedDeviceRefreshDevice {
  deviceId: string
  displayName: string
  state:
    | 'discovered'
    | 'connecting'
    | 'connected'
    | 'already_present'
    | 'waiting_for_peer'
    | 'waiting_for_update'
    | 'version_incompatible'
    | 'rejected'
}

export interface OhSharedDeviceRefresh {
  requestId: string
  phase: 'started' | 'discovering' | 'connecting' | 'round_completed'
  devices: OhSharedDeviceRefreshDevice[]
  totalCount: number
  discoveredCount: number
  connectingCount: number
  connectedCount: number
  alreadyPresentCount: number
  waitingForPeerCount: number
  waitingForUpdateCount: number
  versionIncompatibleCount: number
  rejectedCount: number
  unavailableSourceCount: number
}

export interface OhMemberRemoval {
  phase: 'applied' | 'converging' | 'complete' | 'recovery_required'
  intentCount: number
  effectiveMemberCount: number
  convergenceDigest?: string
  updatedAtMs: number
}

export interface OhEngineEvent {
  kind: string
  state?: string
  refreshReason?: string
  operationId?: string
  terminal?: string
  lifecycleAction?: string
  errorCode?: number
  errorCategory?: string
  retryable?: boolean
  memberRemoval?: OhMemberRemoval
  sharedDeviceRefresh?: OhSharedDeviceRefresh
  networkRecoveryPhase?: 'idle' | 'recovering' | 'retry_scheduled' | 'failed'
  nextRetryInMs?: number
  rePairingScope?: 'all_devices'
}

export interface OhSpaceCreated {
  spaceId: string
  selfDeviceId: string
  identityFingerprint: string
}

export interface OhActiveClipboard {
  entryId: string
  activatedBy: string
}

export interface OhEngine {
  createSpace(deviceName: string | null, passphrase: string): Promise<OhSpaceCreated>
  recoverSession(allowSecureStorageUnlock: boolean): Promise<OhSessionRecovery>
  recoverNetwork(): Promise<void>
  queryNetworkRecoveryStatus(): Promise<OhNetworkRecoveryStatus>
  queryLocalDevice(): Promise<OhLocalDevice>
  queryDeviceGroupChoices(): Promise<string>
  queryMembershipConvergence(): Promise<OhMembershipConvergence>
  issueInvitation(): Promise<OhInvitationIssued>
  joinSpace(
    invitationCode: string,
    deviceName: string | null,
    passphrase: string,
    preserveUnreadableHistory: boolean
  ): Promise<OhJoinSpaceStatus>
  cancelJoinSpace(joinId: string): Promise<OhJoinSpaceStatus>
  refreshSharedDevices(): Promise<OhSharedDeviceRefreshStarted>
  querySharedDeviceRefresh(requestId: string): Promise<OhSharedDeviceRefresh | null>
  removeMember(deviceId: string): Promise<OhMemberRemoval>
  queryMemberRemoval(): Promise<OhMemberRemoval>
  queryActiveClipboard(): Promise<OhActiveClipboard | null>
  lifecycleState(): Promise<string>
  suspend(): Promise<void>
  resume(): Promise<void>
  sendText(text: string, targetDevices: string[]): Promise<OhSendReport>
  exportEntry(entryId: string, destinationHandle: string): Promise<void>
  nextEvent(timeoutMs: number): Promise<OhEngineEvent | null>
  shutdown(deadlineMs: number): Promise<void>
}


// 本地诊断计数使用十进制字符串；只有当前进程的刷新得到确认。
export enum OhHostDiagnosticSource { Application, ShareExtension, KeyboardExtension, BackgroundService }
export enum OhHostDiagnosticAction { RuntimeStart, RuntimeStop, OwnershipAcquire, SecurityPrepare }
export enum OhHostDiagnosticFailure { Unavailable, PermissionDenied, Locked, Busy, Unknown }
export enum OhHostLifecycleState { Foreground, Background }
export enum OhHostNetworkKind { Wifi, Cellular, Ethernet, Other, Unknown }
export enum OhSourceCapability { Supported, Partial, Unsupported, Unknown }
export enum OhHostDiagnosticOutcome { Completed, Failed, Interrupted }
export interface OhLocalCaptureStatus {
  mode: string
  captureId?: string
  remainingMs: number
  startedAtUtc?: string
  endReason?: string
  lastCaptureId?: string
  revision: string
}
export interface OhSourceCoverage {
  source: string
  capability: string
  collection: string
  observedCount: string
  policyFilteredCount: string
}
export interface OhFileSourceCounts {
  source: string
  acceptedCount: string
  writtenCount: string
  queueDroppedCount: string
  quotaDroppedCount: string
  writeFailedCount: string
  lastWrittenAtMs?: string
}
export interface OhLocalDiagnosticStatus {
  runId: string
  capture: OhLocalCaptureStatus
  observedRecords: string
  policyFilteredRecords: string
  schemaRejectedRecords: string
  correlationLimitedRecords: string
  engineVersion: string
  sourceCommit: string
  counterScope: string
  sources: OhSourceCoverage[]
  localFile: string
  closed: boolean
}
export interface OhLocalDiagnosticExportReport {
  flush: string
  status: OhLocalDiagnosticStatus
  requestedAtUtc: string
  completedAtUtc: string
  otherProcessesFlushed: boolean
  files: OhFileSourceCounts[]
}
export interface OhHostDiagnosticReceipt {
  status: string
  token?: string
}

declare const engine: {
  startLocalDiagnosticCapture(durationMs: number): OhLocalCaptureStatus
  stopLocalDiagnosticCapture(captureId: string): string
  queryLocalDiagnosticStatus(): OhLocalDiagnosticStatus
  prepareLocalDiagnosticExport(deadlineMs: number): Promise<OhLocalDiagnosticExportReport>
  registerHostDiagnosticSource(source: OhHostDiagnosticSource, capability: OhSourceCapability): void
  beginHostDiagnostic(source: OhHostDiagnosticSource, action: OhHostDiagnosticAction): OhHostDiagnosticReceipt
  finishHostDiagnostic(source: OhHostDiagnosticSource, token: string, outcome: OhHostDiagnosticOutcome, reason?: OhHostDiagnosticFailure): OhHostDiagnosticReceipt
  recordHostLifecycle(source: OhHostDiagnosticSource, state: OhHostLifecycleState): OhHostDiagnosticReceipt
  recordHostNetworkChange(source: OhHostDiagnosticSource, kind: OhHostNetworkKind, available: boolean): OhHostDiagnosticReceipt
  recordHostOwnershipReleased(source: OhHostDiagnosticSource): OhHostDiagnosticReceipt
  coreVersion(): string
  installProcessObservability(
    config: OhObservabilityConfig,
    directories: OhHostDirectories
  ): OhObservabilitySetup
  queryProcessObservabilityHealth(): OhObservabilityHealth
  flushProcessObservability(deadlineMs: number): Promise<OhObservabilitySignalSummary>
  shutdownProcessObservability(deadlineMs: number): Promise<OhObservabilitySignalSummary>
  prepareHost(host: OhHost): PreparedHost
  startEngine(
    config: { appVersion: string; profileId: string },
    preparedHost: PreparedHost
  ): Promise<OhEngine>
}

export default engine
