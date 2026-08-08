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
  queryMembershipConvergence(): Promise<OhMembershipConvergence>
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

declare const engine: {
  coreVersion(): string
  prepareHost(host: OhHost): PreparedHost
  startEngine(
    config: { appVersion: string; profileId: string },
    preparedHost: PreparedHost
  ): Promise<OhEngine>
}

export default engine
