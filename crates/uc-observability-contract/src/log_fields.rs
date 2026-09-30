//! 模块日志字段目录：每个字段名对应一个值类别，记录点只能写目录内的字段名和该类别接受的值。
//!
//! 字段名被展开为真实的模块路径（`fields::<名称>`），未登记的名字是编译错误；值类别由 [`Accept`]
//! 的实现集合决定，类别不匹配同样是编译错误。运行期文本字段白名单由本目录派生，不再另行手写。
//!
//! 迁移期间目录只包含已迁移到 `uc_*!` 宏的字段，其余字段仍在运行期 crate 的过渡清单里；
//! 字段每迁移一个就从过渡清单搬到这里。

use std::fmt::{Debug, Display};
use std::io;

use tracing::field::{debug, display, DebugValue, DisplayValue, Value};

/// 固定词表：`&'static str` 字面量，或为自己实现 `Accept<Literal>` 的封闭枚举。
pub struct Literal;
/// 应用生成的随机标识，必须经 [`log_id`] 显式适配。
pub struct Identifier;
/// 已审定的词表类文本（枚举名、表名、MIME 等，不含用户数据），字面量直接接受，其余经 [`log_vocab`] 或 [`log_vocab_debug`] 显式适配。
pub struct Vocabulary;
/// 计数、字节数、时长、布尔值等数值或布尔值，原样写出。
pub struct Scalar;
/// [`crate::error_source::io_error_kind`] 的结果。
pub struct IoKind;

/// 值类别在目录中的运行期表示，用于派生白名单。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FieldClass {
    Literal,
    Identifier,
    Vocabulary,
    Scalar,
    IoKind,
}

impl FieldClass {
    /// 是否落盘文本：数字与布尔字段不受文本白名单约束。
    pub const fn is_text(self) -> bool {
        matches!(
            self,
            Self::Literal | Self::Identifier | Self::Vocabulary | Self::IoKind
        )
    }
}

/// 目录中的一个字段。
#[derive(Clone, Copy, Debug)]
pub struct CatalogField {
    pub name: &'static str,
    pub class: FieldClass,
}

/// 字段接受某一类别的值，并转成 tracing 可记录的值。
#[diagnostic::on_unimplemented(
    message = "this value is not accepted by the log field's declared class",
    label = "not accepted here",
    note = "fixed-vocabulary fields take literals or closed enums; identifier fields take `log_id(&value)`; \
            see `uc_observability_contract::log_fields`"
)]
pub trait Accept<Class> {
    type Out: Value;

    fn accept(self) -> Self::Out;
}

/// 固定词表：字面量直接接受；封闭枚举各自为自己实现 `Accept<Literal>`，返回固定文本。
impl Accept<Literal> for &'static str {
    type Out = &'static str;

    fn accept(self) -> &'static str {
        self
    }
}

impl<T: Accept<Literal>> Accept<Literal> for Option<T> {
    type Out = Option<T::Out>;

    fn accept(self) -> Self::Out {
        match self {
            Some(value) => Some(value.accept()),
            None => None,
        }
    }
}

/// 显式声明“这是应用生成的随机标识”的适配器，构造点即评审点。
pub struct Id<'a>(&'a dyn Display);

pub fn log_id(value: &dyn Display) -> Id<'_> {
    Id(value)
}

impl<'a> Accept<Identifier> for Id<'a> {
    type Out = DisplayValue<&'a dyn Display>;

    fn accept(self) -> Self::Out {
        display(self.0)
    }
}

/// 显式声明“这是已审定的词表类文本”的适配器（`Display` 形式），构造点即评审点。
pub struct Vocab<'a>(&'a dyn Display);

pub fn log_vocab(value: &dyn Display) -> Vocab<'_> {
    Vocab(value)
}

impl<'a> Accept<Vocabulary> for Vocab<'a> {
    type Out = DisplayValue<&'a dyn Display>;

    fn accept(self) -> Self::Out {
        display(self.0)
    }
}

/// 同上，`Debug` 形式，用于枚举名。
pub struct VocabDebug<'a>(&'a dyn Debug);

pub fn log_vocab_debug(value: &dyn Debug) -> VocabDebug<'_> {
    VocabDebug(value)
}

impl<'a> Accept<Vocabulary> for VocabDebug<'a> {
    type Out = DebugValue<&'a dyn Debug>;

    fn accept(self) -> Self::Out {
        debug(self.0)
    }
}

impl Accept<Vocabulary> for &'static str {
    type Out = &'static str;

    fn accept(self) -> &'static str {
        self
    }
}

impl<T: Accept<Vocabulary>> Accept<Vocabulary> for Option<T> {
    type Out = Option<T::Out>;

    fn accept(self) -> Self::Out {
        match self {
            Some(value) => Some(value.accept()),
            None => None,
        }
    }
}

macro_rules! accept_as_is {
    ($class:ty: $($value:ty),+ $(,)?) => {
        $(impl Accept<$class> for $value {
            type Out = $value;

            fn accept(self) -> $value {
                self
            }
        })+
    };
}

accept_as_is!(Scalar: u8, u16, u32, u64, u128, usize, i8, i16, i32, i64, i128, isize, f32, f64);
accept_as_is!(Scalar: bool);
accept_as_is!(Vocabulary: u8, u16, u32, u64, i32, i64, usize, bool);

impl<T: Accept<Scalar>> Accept<Scalar> for Option<T> {
    type Out = Option<T::Out>;

    fn accept(self) -> Self::Out {
        match self {
            Some(value) => Some(value.accept()),
            None => None,
        }
    }
}

impl<'a, T: Accept<Scalar> + Copy> Accept<Scalar> for &'a T {
    type Out = T::Out;

    fn accept(self) -> Self::Out {
        (*self).accept()
    }
}

impl Accept<IoKind> for Option<DebugValue<io::ErrorKind>> {
    type Out = Self;

    fn accept(self) -> Self {
        self
    }
}

/// 错误字段：沿用既有错误链渲染，只接受 `&dyn Error`。
pub fn error_field<'a>(
    error: &'a (dyn std::error::Error + 'static),
) -> &'a (dyn std::error::Error + 'static) {
    error
}

#[doc(hidden)]
#[macro_export]
macro_rules! __log_field_ack {
    (Identifier(random)) => {};
    (Vocabulary(reviewed)) => {};
    (Literal) => {};
    (Scalar) => {};
    (IoKind) => {};
}

/// 声明字段目录：每项 `名称: 类别`；`Identifier` 必须写成 `Identifier(random)`，确认取值由应用随机生成。
#[doc(hidden)]
#[macro_export]
macro_rules! __log_field_catalog {
    ($($name:ident : $class:ident $(($ack:ident))?),* $(,)?) => {
        $($crate::__log_field_ack!($class $(($ack))?);)*
        pub mod fields {
            $(pub mod $name {
                /// 该字段声明的值类别。
                pub type Class = $crate::log_fields::$class;
            })*
        }
        pub const CATALOG: &[$crate::log_fields::CatalogField] = &[
            $($crate::log_fields::CatalogField {
                name: stringify!($name),
                class: $crate::log_fields::FieldClass::$class,
            }),*
        ];
    };
}

__log_field_catalog! {
    // Identifier
    attempt_id: Identifier(random),
    blob_id: Identifier(random),
    entry_id: Identifier(random),
    entry_id_str: Identifier(random),
    event_id: Identifier(random),
    existing_entry_id: Identifier(random),
    paste_rep_id: Identifier(random),
    plain_rep_id: Identifier(random),
    preview_rep_id: Identifier(random),
    rep_id: Identifier(random),
    representation_id: Identifier(random),
    transfer_id: Identifier(random),
    // Literal
    cause: Literal,
    context: Literal,
    dependency: Literal,
    emitter: Literal,
    error_kind: Literal,
    error_stage: Literal,
    event: Literal,
    failure: Literal,
    failure_stage: Literal,
    file_paths_source: Literal,
    issue: Literal,
    msg_kind: Literal,
    plan: Literal,
    reject_reason: Literal,
    rollback_target: Literal,
    step: Literal,
    storage_generation: Literal,
    strategy: Literal,
    task: Literal,
    trigger: Literal,
    worker: Literal,
    // Vocabulary
    ack_kind: Vocabulary(reviewed),
    alpn: Vocabulary(reviewed),
    category: Vocabulary(reviewed),
    congestion_controller: Vocabulary(reviewed),
    current: Vocabulary(reviewed),
    current_version: Vocabulary(reviewed),
    doc_table: Vocabulary(reviewed),
    error_category: Vocabulary(reviewed),
    error_code: Vocabulary(reviewed),
    error_type: Vocabulary(reviewed),
    evaluation: Vocabulary(reviewed),
    event_kind: Vocabulary(reviewed),
    existing_status: Vocabulary(reviewed),
    expected: Vocabulary(reviewed),
    failure_reason: Vocabulary(reviewed),
    filter_kind: Vocabulary(reviewed),
    format_id: Vocabulary(reviewed),
    format_ids: Vocabulary(reviewed),
    intent: Vocabulary(reviewed),
    key_class: Vocabulary(reviewed),
    kind: Vocabulary(reviewed),
    mime: Vocabulary(reviewed),
    mimes: Vocabulary(reviewed),
    mode: Vocabulary(reviewed),
    next_phase: Vocabulary(reviewed),
    op: Vocabulary(reviewed),
    operation: Vocabulary(reviewed),
    origin: Vocabulary(reviewed),
    original_mime: Vocabulary(reviewed),
    outcome: Vocabulary(reviewed),
    packed_rep_ids: Vocabulary(reviewed),
    payload_state: Vocabulary(reviewed),
    phase: Vocabulary(reviewed),
    posting_table: Vocabulary(reviewed),
    previous_phase: Vocabulary(reviewed),
    reason: Vocabulary(reviewed),
    recovery_state: Vocabulary(reviewed),
    reply_kind: Vocabulary(reviewed),
    result: Vocabulary(reviewed),
    rules: Vocabulary(reviewed),
    scope: Vocabulary(reviewed),
    source: Vocabulary(reviewed),
    stage: Vocabulary(reviewed),
    state: Vocabulary(reviewed),
    stored_version: Vocabulary(reviewed),
    table: Vocabulary(reviewed),
    uc_congestion_controller: Vocabulary(reviewed),
    upgrade_action: Vocabulary(reviewed),
    upgrade_phase: Vocabulary(reviewed),
    variant: Vocabulary(reviewed),
    version: Vocabulary(reviewed),
    // IoKind
    io_error_kind: IoKind,
    // Scalar
    accepted: Scalar,
    active_member_count: Scalar,
    add_bytes_ms: Scalar,
    add_path_ms: Scalar,
    addr_count: Scalar,
    advanced: Scalar,
    affected_device_count: Scalar,
    allow_overlay: Scalar,
    allow_overlay_network_addrs: Scalar,
    allow_relay_fallback: Scalar,
    already_initialized: Scalar,
    attempt: Scalar,
    backoff_ms: Scalar,
    baseline_ms: Scalar,
    blob_ref_count: Scalar,
    budget_ms: Scalar,
    bytes: Scalar,
    bytes_downloaded: Scalar,
    bytes_len: Scalar,
    bytes_reclaimed: Scalar,
    bytes_reclaimed_mb: Scalar,
    bytes_written: Scalar,
    cache_bytes: Scalar,
    cache_entries_deleted: Scalar,
    cache_files_removed: Scalar,
    cache_hit: Scalar,
    cache_orphans_removed: Scalar,
    can_submit_passphrase: Scalar,
    cancelled: Scalar,
    candidates: Scalar,
    cap: Scalar,
    ciphertext_bytes: Scalar,
    circuit_tripped: Scalar,
    cleanup_failed: Scalar,
    code: Scalar,
    completed: Scalar,
    completed_peer_count: Scalar,
    compressed_size: Scalar,
    connect_ms: Scalar,
    consecutive_failures: Scalar,
    converted_size: Scalar,
    cooldown_ms: Scalar,
    corrupted: Scalar,
    count: Scalar,
    current_epoch: Scalar,
    custom_relay_count: Scalar,
    database_bytes: Scalar,
    decrypted: Scalar,
    decrypted_bytes: Scalar,
    deferred_count: Scalar,
    deferred_peer_count: Scalar,
    deleted: Scalar,
    demoted: Scalar,
    device_count: Scalar,
    directory_available: Scalar,
    disable_relays: Scalar,
    download_ms: Scalar,
    dropped_count: Scalar,
    dropped_reps: Scalar,
    duplicate: Scalar,
    elapsed_ms: Scalar,
    encrypted: Scalar,
    encryption_initialized: Scalar,
    entries_deleted: Scalar,
    entries_scanned: Scalar,
    envelope_len: Scalar,
    errored: Scalar,
    errors: Scalar,
    evicted_count: Scalar,
    expected_schema_ver: Scalar,
    expired_files: Scalar,
    export_ms: Scalar,
    extracted_paths_count: Scalar,
    failed: Scalar,
    file_candidate_count: Scalar,
    file_size: Scalar,
    files_removed: Scalar,
    found_schema_ver: Scalar,
    freed_bytes: Scalar,
    gc_interval_secs: Scalar,
    grace_ms: Scalar,
    grandfathered: Scalar,
    group_epoch: Scalar,
    has_completed: Scalar,
    has_current_invitation: Scalar,
    has_device_name: Scalar,
    has_kek: Scalar,
    has_more: Scalar,
    has_space: Scalar,
    hash_ms: Scalar,
    healthy: Scalar,
    idx: Scalar,
    indexed: Scalar,
    indexed_paths: Scalar,
    ingest_failed: Scalar,
    inline: Scalar,
    io_error_code: Scalar,
    ip_addr_count: Scalar,
    is_favorited: Scalar,
    last_attempt_ms: Scalar,
    latency_ms: Scalar,
    limit: Scalar,
    line_count: Scalar,
    line_index: Scalar,
    list_failed: Scalar,
    local_path_count: Scalar,
    logs_bytes: Scalar,
    managed_total_mb: Scalar,
    max: Scalar,
    max_attempts: Scalar,
    max_bytes: Scalar,
    missed: Scalar,
    missing: Scalar,
    missing_count: Scalar,
    missing_entries_deleted: Scalar,
    ms: Scalar,
    new_effect_count: Scalar,
    next_recheck_ms: Scalar,
    now_ms: Scalar,
    offline: Scalar,
    offset: Scalar,
    on_disk_size: Scalar,
    original_size: Scalar,
    orphan_count: Scalar,
    orphaned: Scalar,
    orphans_removed: Scalar,
    os_write_succeeded: Scalar,
    packed_rep_count: Scalar,
    page_count: Scalar,
    page_number: Scalar,
    partial: Scalar,
    partial_publication: Scalar,
    pending: Scalar,
    pending_group_update_count: Scalar,
    plaintext_bytes: Scalar,
    plaintext_len: Scalar,
    plaintext_size: Scalar,
    port: Scalar,
    preview_length_chars: Scalar,
    previous_epoch: Scalar,
    profile_ready: Scalar,
    projections: Scalar,
    proof_len: Scalar,
    provider_failures: Scalar,
    provisional_discarded: Scalar,
    publish_ms: Scalar,
    published: Scalar,
    quota_mb: Scalar,
    received_page_count: Scalar,
    reconcile_failed: Scalar,
    reconciled: Scalar,
    recovered: Scalar,
    recovered_after_failures: Scalar,
    relay_url_count: Scalar,
    remaining: Scalar,
    remaining_secs: Scalar,
    removed: Scalar,
    rendezvous_override: Scalar,
    rep_count: Scalar,
    repr_count: Scalar,
    representation_count: Scalar,
    representation_index: Scalar,
    representations: Scalar,
    restart_required: Scalar,
    retention_entries_deleted: Scalar,
    retention_failed: Scalar,
    retryable: Scalar,
    returned: Scalar,
    reused_existing: Scalar,
    rewritten: Scalar,
    rewritten_rep_count: Scalar,
    rolled_back: Scalar,
    root_count: Scalar,
    save_ref_ms: Scalar,
    secret_count: Scalar,
    sender_is_bound: Scalar,
    settled_count: Scalar,
    size: Scalar,
    size_bytes: Scalar,
    size_cap_exceeded: Scalar,
    skipped: Scalar,
    skipped_pipeline: Scalar,
    skipped_projection: Scalar,
    snapshot_rep_count: Scalar,
    stable_failure_count: Scalar,
    staged: Scalar,
    staged_with_preview: Scalar,
    stripped_count: Scalar,
    success: Scalar,
    target_activated: Scalar,
    target_epoch: Scalar,
    threshold: Scalar,
    ticket_ms: Scalar,
    tolerance_ms: Scalar,
    total: Scalar,
    total_bytes: Scalar,
    total_ciphertext_bytes: Scalar,
    total_entries: Scalar,
    total_file_bytes: Scalar,
    total_plaintext_bytes: Scalar,
    total_size_bytes: Scalar,
    tried_providers: Scalar,
    ttl_ms: Scalar,
    ttl_secs: Scalar,
    unsupported_member: Scalar,
    vault_bytes: Scalar,
    visible_roots: Scalar,
}

/// 目录中该名字是否为落盘文本字段。
pub fn text_field_in_catalog(name: &str) -> bool {
    CATALOG
        .iter()
        .any(|field| field.name == name && field.class.is_text())
}
