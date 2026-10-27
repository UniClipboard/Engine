//! 固定宽度字段均用大端；先验证外层大小与页计数，再分配相应缓冲。
use super::model::{ItemStateKind, MessageKind, OutcomeKind, QueryKind};
use super::{
    ActiveProgress, CancelReason, Envelope, FailureReason, ItemKind, ItemState, LogicalItem,
    Message, Phase, ProducerDescriptor, QueryResult, RejectionReason, SnapshotPage, TaskScope,
    TerminalOutcome, TerminalResult, WireId, WireValueError, MAX_FRAME_BYTES, MAX_ITEMS,
    MAX_PAGE_ITEMS,
};
use crate::{WireTraceContext, TRACEPARENT_MAX_BYTES};
use std::collections::HashSet;
use std::{fmt, string::FromUtf8Error};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use uc_core::error_class::ErrorClass;

const MAGIC: &[u8; 3] = b"UCP";
const VERSION: u8 = 2;
const HEADER_BYTES: usize = 8;

// 协议 crate 不依赖观测运行时；拒绝类型、固定显示和错误分类仍只维护一份清单。
macro_rules! frame_refusals {
    ($($variant:ident => ($message:literal, $class:literal)),+ $(,)?) => {
        #[derive(Debug, Error, PartialEq, Eq)]
        pub enum FrameRefusal { $(#[error($message)] $variant),+ }

        impl ErrorClass for FrameRefusal {
            fn class(&self) -> &'static str {
                match self { $(Self::$variant => $class),+ }
            }
        }
    };
}

frame_refusals! {
    Magic => ("invalid transfer progress magic", "transfer_progress_magic"),
    Version => ("unsupported transfer progress version", "transfer_progress_version"),
    Length => ("invalid transfer progress frame length", "transfer_progress_length"),
    Truncated => ("truncated transfer progress frame", "transfer_progress_truncated"),
    TrailingBytes => ("trailing transfer progress bytes", "transfer_progress_trailing_bytes"),
    Page => ("invalid transfer progress page", "transfer_progress_page"),
    Item => ("invalid transfer progress item", "transfer_progress_item"),
    Option => ("invalid transfer progress option", "transfer_progress_option"),
    TraceContext => ("invalid transfer progress trace carrier", "transfer_progress_trace_context"),
}

#[derive(Error)]
pub enum WireError {
    #[error("transfer progress stream failed")]
    Io(#[from] std::io::Error),
    #[error("transfer progress frame rejected")]
    Frame(#[from] FrameRefusal),
    #[error("transfer progress value rejected")]
    Value(#[from] WireValueError),
    #[error("transfer progress trace carrier is not utf8")]
    Utf8(#[from] FromUtf8Error),
}

impl fmt::Debug for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, formatter)
    }
}

impl ErrorClass for WireError {
    fn class(&self) -> &'static str {
        match self {
            Self::Io(_) => "transfer_progress_io",
            Self::Frame(source) => source.class(),
            Self::Value(source) => source.class(),
            Self::Utf8(_) => "transfer_progress_trace_utf8",
        }
    }
}

struct Encoder(Vec<u8>);

impl Encoder {
    fn id(&mut self, id: WireId) {
        self.0.extend_from_slice(&id.0);
    }
    fn u16(&mut self, value: u16) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.0.extend_from_slice(&value.to_be_bytes());
    }
    fn optional_total(&mut self, value: Option<u64>) {
        self.0.push(u8::from(value.is_some()));
        if let Some(value) = value {
            self.u64(value);
        }
    }
    fn scope(&mut self, scope: TaskScope) {
        self.id(scope.source_entry);
        self.id(scope.attempt);
    }
    fn descriptor(&mut self, descriptor: &ProducerDescriptor) {
        self.id(descriptor.producer);
        self.u64(descriptor.owner_revision);
        self.scope(descriptor.scope);
        self.u16(descriptor.item_count);
    }
    fn item(&mut self, item: LogicalItem) {
        self.0.push(item.kind as u8);
        self.u16(item.index);
    }
    fn active(&mut self, value: &ActiveProgress) {
        self.item(value.item);
        self.u32(value.round);
        self.u64(value.sequence);
        self.u64(value.bytes);
        self.optional_total(value.total);
        self.0.push(value.phase as u8);
    }
    fn terminal(&mut self, value: &TerminalResult) {
        self.item(value.item);
        self.u32(value.round);
        self.u64(value.sequence);
        self.u64(value.result_revision);
        self.u64(value.bytes);
        self.optional_total(value.total);
        match value.outcome {
            TerminalOutcome::Completed => self.0.push(OutcomeKind::Completed as u8),
            TerminalOutcome::Failed(reason) => {
                self.0.push(OutcomeKind::Failed as u8);
                self.0.push(reason as u8);
            }
            TerminalOutcome::Cancelled(reason) => {
                self.0.push(OutcomeKind::Cancelled as u8);
                self.0.push(reason as u8);
            }
        }
    }
    fn page(&mut self, page: &SnapshotPage) {
        self.u64(page.snapshot_revision);
        self.u16(page.offset);
        self.u16(page.total);
        // validate_message 已在进入编码之前证明长度不超过64。
        self.u16(page.items.len() as u16);
        for item in &page.items {
            match item {
                ItemState::Active(value) => {
                    self.0.push(ItemStateKind::Active as u8);
                    self.active(value);
                }
                ItemState::Terminal(value) => {
                    self.0.push(ItemStateKind::Terminal as u8);
                    self.terminal(value);
                }
            }
        }
    }
}

pub fn encode_frame(envelope: &Envelope) -> Result<Vec<u8>, WireError> {
    let message = &envelope.message;
    validate_message(message)?;
    if envelope
        .trace_context
        .as_ref()
        .is_some_and(|context| !context.is_bounded())
    {
        return Err(FrameRefusal::TraceContext.into());
    }
    let mut body = Encoder(Vec::new());
    body.0.push(u8::from(envelope.trace_context.is_some()));
    if let Some(context) = &envelope.trace_context {
        // 上界已证明为256，长度转换不会截断。
        body.u16(context.traceparent.len() as u16);
        body.0.extend_from_slice(context.traceparent.as_bytes());
    }
    match message {
        Message::Register { source_entry } => {
            body.0.push(MessageKind::Register as u8);
            body.id(*source_entry);
        }
        Message::Challenge { nonce } => {
            body.0.push(MessageKind::Challenge as u8);
            body.id(*nonce);
        }
        Message::CurrentDescriptor {
            challenge,
            descriptor,
        } => {
            body.0.push(MessageKind::CurrentDescriptor as u8);
            body.id(*challenge);
            body.descriptor(descriptor);
        }
        Message::Bound {
            challenge,
            binding,
            descriptor,
        } => {
            body.0.push(MessageKind::Bound as u8);
            body.id(*challenge);
            body.id(*binding);
            body.descriptor(descriptor);
        }
        Message::Update {
            binding,
            scope,
            owner_revision,
            progress,
        } => {
            body.0.push(MessageKind::Update as u8);
            body.id(*binding);
            body.scope(*scope);
            body.u64(*owner_revision);
            body.active(progress);
        }
        Message::Contact {
            binding,
            scope,
            owner_revision,
            sequence,
        } => {
            body.0.push(MessageKind::Contact as u8);
            body.id(*binding);
            body.scope(*scope);
            body.u64(*owner_revision);
            body.u64(*sequence);
        }
        Message::Terminal {
            binding,
            scope,
            owner_revision,
            result,
        }
        | Message::TerminalAck {
            binding,
            scope,
            owner_revision,
            result,
        } => {
            body.0.push(if matches!(message, Message::Terminal { .. }) {
                MessageKind::Terminal as u8
            } else {
                MessageKind::TerminalAck as u8
            });
            body.id(*binding);
            body.scope(*scope);
            body.u64(*owner_revision);
            body.terminal(result);
        }
        Message::Query {
            challenge,
            source_entry,
            expected_attempt,
            offset,
            snapshot_revision,
        } => {
            body.0.push(MessageKind::Query as u8);
            body.id(*challenge);
            body.id(*source_entry);
            body.0.push(u8::from(expected_attempt.is_some()));
            if let Some(attempt) = expected_attempt {
                body.id(*attempt);
            }
            body.u16(*offset);
            body.optional_total(*snapshot_revision);
        }
        Message::QueryResult { challenge, result } => {
            body.0.push(MessageKind::QueryResult as u8);
            body.id(*challenge);
            match result {
                QueryResult::Current { descriptor, page }
                | QueryResult::Superseded { descriptor, page } => {
                    body.0
                        .push(if matches!(result, QueryResult::Current { .. }) {
                            QueryKind::Current as u8
                        } else {
                            QueryKind::Superseded as u8
                        });
                    body.descriptor(descriptor);
                    body.page(page);
                }
                QueryResult::NotRetained => body.0.push(QueryKind::NotRetained as u8),
                QueryResult::Unsupported => body.0.push(QueryKind::Unsupported as u8),
                QueryResult::AccessDenied => body.0.push(QueryKind::AccessDenied as u8),
            }
        }
        Message::Rejected { challenge, reason } => {
            body.0.push(MessageKind::Rejected as u8);
            body.id(*challenge);
            body.0.push(*reason as u8);
        }
    }
    if body.0.is_empty() || body.0.len() > MAX_FRAME_BYTES - HEADER_BYTES {
        return Err(FrameRefusal::Length.into());
    }
    let mut frame = Vec::with_capacity(HEADER_BYTES + body.0.len());
    frame.extend_from_slice(MAGIC);
    frame.push(VERSION);
    frame.extend_from_slice(&(body.0.len() as u32).to_be_bytes());
    frame.extend(body.0);
    Ok(frame)
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn fixed<const N: usize>(&mut self) -> Result<[u8; N], WireError> {
        let source = self.0.get(..N).ok_or(FrameRefusal::Truncated)?;
        let mut value = [0; N];
        value.copy_from_slice(source);
        self.0 = &self.0[N..];
        Ok(value)
    }
    fn byte(&mut self) -> Result<u8, WireError> {
        Ok(self.fixed::<1>()?[0])
    }
    fn id(&mut self) -> Result<WireId, WireError> {
        Ok(WireId(self.fixed()?))
    }
    fn u16(&mut self) -> Result<u16, WireError> {
        Ok(u16::from_be_bytes(self.fixed()?))
    }
    fn u32(&mut self) -> Result<u32, WireError> {
        Ok(u32::from_be_bytes(self.fixed()?))
    }
    fn u64(&mut self) -> Result<u64, WireError> {
        Ok(u64::from_be_bytes(self.fixed()?))
    }
    fn presence(&mut self) -> Result<bool, WireError> {
        match self.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(FrameRefusal::Option.into()),
        }
    }
    fn optional_total(&mut self) -> Result<Option<u64>, WireError> {
        if self.presence()? {
            Ok(Some(self.u64()?))
        } else {
            Ok(None)
        }
    }
    fn scope(&mut self) -> Result<TaskScope, WireError> {
        Ok(TaskScope {
            source_entry: self.id()?,
            attempt: self.id()?,
        })
    }
    fn descriptor(&mut self) -> Result<ProducerDescriptor, WireError> {
        let value = ProducerDescriptor {
            producer: self.id()?,
            owner_revision: self.u64()?,
            scope: self.scope()?,
            item_count: self.u16()?,
        };
        validate_descriptor(&value)?;
        Ok(value)
    }
    fn item(&mut self) -> Result<LogicalItem, WireError> {
        let item = LogicalItem {
            kind: ItemKind::try_from(self.byte()?)?,
            index: self.u16()?,
        };
        validate_item(item)?;
        Ok(item)
    }
    fn active(&mut self) -> Result<ActiveProgress, WireError> {
        Ok(ActiveProgress {
            item: self.item()?,
            round: self.u32()?,
            sequence: self.u64()?,
            bytes: self.u64()?,
            total: self.optional_total()?,
            phase: Phase::try_from(self.byte()?)?,
        })
    }
    fn terminal(&mut self) -> Result<TerminalResult, WireError> {
        let item = self.item()?;
        let round = self.u32()?;
        let sequence = self.u64()?;
        let result_revision = self.u64()?;
        let bytes = self.u64()?;
        let total = self.optional_total()?;
        let outcome = match OutcomeKind::try_from(self.byte()?)? {
            OutcomeKind::Completed => TerminalOutcome::Completed,
            OutcomeKind::Failed => TerminalOutcome::Failed(FailureReason::try_from(self.byte()?)?),
            OutcomeKind::Cancelled => {
                TerminalOutcome::Cancelled(CancelReason::try_from(self.byte()?)?)
            }
        };
        Ok(TerminalResult {
            item,
            round,
            sequence,
            result_revision,
            bytes,
            total,
            outcome,
        })
    }
    fn page(&mut self) -> Result<SnapshotPage, WireError> {
        let snapshot_revision = self.u64()?;
        let offset = self.u16()?;
        let total = self.u16()?;
        let count = usize::from(self.u16()?);
        validate_page_bounds(offset, total, count)?;
        let mut items = Vec::with_capacity(count);
        for _ in 0..count {
            items.push(match ItemStateKind::try_from(self.byte()?)? {
                ItemStateKind::Active => ItemState::Active(self.active()?),
                ItemStateKind::Terminal => ItemState::Terminal(self.terminal()?),
            });
        }
        Ok(SnapshotPage {
            snapshot_revision,
            offset,
            total,
            items,
        })
    }
}

fn frame_length(header: &[u8; HEADER_BYTES]) -> Result<usize, WireError> {
    if header[..3] != MAGIC[..] {
        return Err(FrameRefusal::Magic.into());
    }
    if header[3] != VERSION {
        return Err(FrameRefusal::Version.into());
    }
    let mut length = [0; 4];
    length.copy_from_slice(&header[4..]);
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES - HEADER_BYTES {
        return Err(FrameRefusal::Length.into());
    }
    Ok(length)
}

fn decode_body(body: &[u8]) -> Result<Envelope, WireError> {
    let mut read = Reader(body);
    let trace_context = if read.presence()? {
        let length = usize::from(read.u16()?);
        if length == 0 || length > TRACEPARENT_MAX_BYTES {
            return Err(FrameRefusal::TraceContext.into());
        }
        let bytes = read.0.get(..length).ok_or(FrameRefusal::Truncated)?;
        let traceparent = String::from_utf8(bytes.to_vec())?;
        read.0 = &read.0[length..];
        Some(WireTraceContext { traceparent })
    } else {
        None
    };
    let kind = MessageKind::try_from(read.byte()?)?;
    let message = match kind {
        MessageKind::Register => Message::Register {
            source_entry: read.id()?,
        },
        MessageKind::Challenge => Message::Challenge { nonce: read.id()? },
        MessageKind::CurrentDescriptor => Message::CurrentDescriptor {
            challenge: read.id()?,
            descriptor: read.descriptor()?,
        },
        MessageKind::Bound => Message::Bound {
            challenge: read.id()?,
            binding: read.id()?,
            descriptor: read.descriptor()?,
        },
        MessageKind::Update => Message::Update {
            binding: read.id()?,
            scope: read.scope()?,
            owner_revision: read.u64()?,
            progress: read.active()?,
        },
        MessageKind::Contact => Message::Contact {
            binding: read.id()?,
            scope: read.scope()?,
            owner_revision: read.u64()?,
            sequence: read.u64()?,
        },
        MessageKind::Terminal | MessageKind::TerminalAck => {
            let binding = read.id()?;
            let scope = read.scope()?;
            let owner_revision = read.u64()?;
            let result = read.terminal()?;
            if kind == MessageKind::Terminal {
                Message::Terminal {
                    binding,
                    scope,
                    owner_revision,
                    result,
                }
            } else {
                Message::TerminalAck {
                    binding,
                    scope,
                    owner_revision,
                    result,
                }
            }
        }
        MessageKind::Query => Message::Query {
            challenge: read.id()?,
            source_entry: read.id()?,
            expected_attempt: if read.presence()? {
                Some(read.id()?)
            } else {
                None
            },
            offset: read.u16()?,
            snapshot_revision: read.optional_total()?,
        },
        MessageKind::QueryResult => {
            let challenge = read.id()?;
            let kind = QueryKind::try_from(read.byte()?)?;
            let result = match kind {
                QueryKind::Current | QueryKind::Superseded => {
                    let descriptor = read.descriptor()?;
                    let page = read.page()?;
                    if kind == QueryKind::Current {
                        QueryResult::Current { descriptor, page }
                    } else {
                        QueryResult::Superseded { descriptor, page }
                    }
                }
                QueryKind::NotRetained => QueryResult::NotRetained,
                QueryKind::Unsupported => QueryResult::Unsupported,
                QueryKind::AccessDenied => QueryResult::AccessDenied,
            };
            Message::QueryResult { challenge, result }
        }
        MessageKind::Rejected => Message::Rejected {
            challenge: read.id()?,
            reason: RejectionReason::try_from(read.byte()?)?,
        },
    };
    if !read.0.is_empty() {
        return Err(FrameRefusal::TrailingBytes.into());
    }
    validate_message(&message)?;
    Ok(Envelope {
        message,
        trace_context,
    })
}

pub fn decode_frame(frame: &[u8]) -> Result<Envelope, WireError> {
    let mut read = Reader(frame);
    let header = read.fixed::<HEADER_BYTES>()?;
    let length = frame_length(&header)?;
    if read.0.len() < length {
        return Err(FrameRefusal::Truncated.into());
    }
    if read.0.len() > length {
        return Err(FrameRefusal::TrailingBytes.into());
    }
    decode_body(read.0)
}

pub async fn write_frame<W: AsyncWrite + Unpin + ?Sized>(
    writer: &mut W,
    envelope: &Envelope,
) -> Result<(), WireError> {
    writer.write_all(&encode_frame(envelope)?).await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin + ?Sized>(
    reader: &mut R,
) -> Result<Envelope, WireError> {
    let mut header = [0; HEADER_BYTES];
    reader.read_exact(&mut header).await?;
    let length = frame_length(&header)?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    decode_body(&body)
}

fn validate_item(item: LogicalItem) -> Result<(), FrameRefusal> {
    if usize::from(item.index) >= MAX_ITEMS
        || (item.kind == ItemKind::DirectoryUnit && item.index != 0)
    {
        return Err(FrameRefusal::Item);
    }
    Ok(())
}

fn validate_descriptor(descriptor: &ProducerDescriptor) -> Result<(), FrameRefusal> {
    if descriptor.item_count == 0 || usize::from(descriptor.item_count) > MAX_ITEMS {
        return Err(FrameRefusal::Page);
    }
    Ok(())
}

fn validate_page_bounds(offset: u16, total: u16, count: usize) -> Result<(), FrameRefusal> {
    if total == 0
        || usize::from(total) > MAX_ITEMS
        || count == 0
        || count > MAX_PAGE_ITEMS
        || usize::from(offset) + count > usize::from(total)
    {
        return Err(FrameRefusal::Page);
    }
    Ok(())
}

fn validate_message(message: &Message) -> Result<(), FrameRefusal> {
    match message {
        Message::CurrentDescriptor { descriptor, .. } | Message::Bound { descriptor, .. } => {
            validate_descriptor(descriptor)
        }
        Message::Update { progress, .. } => validate_item(progress.item),
        Message::Terminal { result, .. } | Message::TerminalAck { result, .. } => {
            validate_item(result.item)
        }
        Message::Query {
            offset,
            snapshot_revision,
            ..
        } => {
            if usize::from(*offset) >= MAX_ITEMS || (*offset != 0 && snapshot_revision.is_none()) {
                return Err(FrameRefusal::Page);
            }
            Ok(())
        }
        Message::QueryResult {
            result:
                QueryResult::Current { descriptor, page } | QueryResult::Superseded { descriptor, page },
            ..
        } => {
            validate_descriptor(descriptor)?;
            validate_page_bounds(page.offset, page.total, page.items.len())?;
            if page.total != descriptor.item_count {
                return Err(FrameRefusal::Page);
            }
            let mut identities = HashSet::with_capacity(page.items.len());
            for state in &page.items {
                let item = match state {
                    ItemState::Active(value) => value.item,
                    ItemState::Terminal(value) => value.item,
                };
                validate_item(item)?;
                if !identities.insert(item) {
                    return Err(FrameRefusal::Page);
                }
            }
            Ok(())
        }
        Message::Register { .. }
        | Message::Challenge { .. }
        | Message::Contact { .. }
        | Message::QueryResult { .. }
        | Message::Rejected { .. } => Ok(()),
    }
}
