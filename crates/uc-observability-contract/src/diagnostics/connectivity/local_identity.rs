//! 本机身份状态变化只记录固定分类，不记录指纹或成员标识。
use super::{emit_local, record::LocalEvent, ObservationContext};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalIdentityState {
    Mismatch,
    Consistent,
}

pub fn record_local_identity_changed(state: LocalIdentityState) {
    emit_local(
        LocalEvent::LocalIdentityChanged { state },
        &ObservationContext::capture(),
    );
}
