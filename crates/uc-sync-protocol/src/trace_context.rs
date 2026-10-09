use serde::{Deserialize, Serialize};

/// 有界 W3C traceparent 载体的最大字节数。
pub const TRACEPARENT_MAX_BYTES: usize = 256;

/// 只在同步协议内部流转的有界 W3C 上下文。
///
/// 本类型只描述线上载体；从当前 span 注入和在远端设置父 span 属于观测装配，
/// 由传输适配层负责。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireTraceContext {
    pub traceparent: String,
}

impl WireTraceContext {
    pub fn is_bounded(&self) -> bool {
        !self.traceparent.is_empty() && self.traceparent.len() <= TRACEPARENT_MAX_BYTES
    }
}
