//! User-opted LAN compatibility workflows for mobile sync
//! (SyncClipboard Clipboard EX).
//!
//! This crate is the dedicated LAN compatibility line (ADR-018 stage 4):
//! it is only compiled when `lan-compat` is explicitly enabled, it is not a
//! dependency of the default P2P path, and P2P failures never fall back to
//! it automatically. The default `uc-application` no longer contains the
//! mobile-sync implementation.

pub(crate) mod deps;
pub(crate) mod facade;
pub(crate) mod usecases;

pub use deps::{MobileDevicePorts, MobileSyncPorts};

pub use facade::{
    ApplyIncomingMobileClipError, ApplyIncomingMobileClipInput, ApplyIncomingMobileClipOutcome,
    AuthenticateBasicAuthError, AuthenticateBasicAuthInput, AuthenticatedDevice,
    BeginMobileFileUpload, CheckContentAvailableError, GetLatestMobileSyncDocError,
    GetMobileSyncFileError, GetMobileSyncFileOutput, GetMobileSyncSettingsError,
    IncomingMobileBuffer, IncomingMobileClipEvent, IsDeviceCredentialCurrentError,
    LanInterfaceOption, ListLanInterfacesError, ListMobileDevicesError, MobileDevicePasswordEdit,
    MobileDeviceSummary, MobileFileUploadError, MobileFileUploadHandle, MobileSyncFacade,
    MobileSyncFacadeDeps, MobileSyncSettingsView, MobileSyncSnapshotPorts,
    RegisterMobileShortcutDeviceError, RegisterMobileShortcutDeviceInput,
    RegisterMobileShortcutDeviceOutput, RevokeMobileDeviceError, RevokeMobileDeviceInput,
    ShortcutInstallMethod, ShortcutInstallMethodOption, SyncClipboardItemType, SyncClipboardMeta,
    UpdateMobileDeviceError, UpdateMobileDeviceInput, UpdateMobileDeviceOutput,
    UpdateMobileSyncSettingsError, UpdateMobileSyncSettingsInput, UpdateMobileSyncSettingsOutput,
    SYNC_CLIPBOARD_EX_INSTALL_URL,
};
