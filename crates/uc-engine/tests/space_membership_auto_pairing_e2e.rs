#![cfg(feature = "dev-tools")]
#![cfg(not(coverage))]

#[path = "space_membership_auto_pairing_e2e/harness/mod.rs"]
mod harness;

#[path = "space_membership_auto_pairing_e2e/six_digit_pairing.rs"]
mod six_digit_pairing;

#[path = "space_membership_auto_pairing_e2e/automatic_connections.rs"]
mod automatic_connections;

#[path = "space_membership_auto_pairing_e2e/removal_convergence.rs"]
mod removal_convergence;

#[path = "space_membership_auto_pairing_e2e/clipboard_trace.rs"]
mod clipboard_trace;

#[path = "space_membership_auto_pairing_e2e/pairing.rs"]
mod pairing;

#[path = "space_membership_auto_pairing_e2e/admission.rs"]
mod admission;

#[path = "space_membership_auto_pairing_e2e/topology.rs"]
mod topology;

#[path = "space_membership_auto_pairing_e2e/space_switch.rs"]
mod space_switch;

#[path = "space_membership_auto_pairing_e2e/membership_history.rs"]
mod membership_history;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::any_value::Value as OtlpValue;
use prost::Message;
use tempfile::TempDir;
use uc_engine::observability::{
    DeploymentEnvironment, LocalLogConfig, ObservabilityConfig, ObservabilityResource,
    OperatingSystem, OtlpHttpConfig, ProcessObservabilityRuntime,
};
use uc_engine::{
    ChooseDeviceGroupInput, CreateSpaceInput, Engine, EngineConfig, HistoryEntryInput,
    HostCapabilities, HostCapabilityError, HostCapabilityErrorCategory, HostClipboard,
    HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle, HostFileMetadata,
    HostSecureStorage, JoinSpaceInput, JoinSpaceStatusSummary, ListHistoryEntriesInput, Operation,
    OperationResult, PairingConfirmationSummary, RemoveMemberInput, SecretString, SendTextInput,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use harness::*;
