use crate::connectivity::PeerConnection;
use std::sync::Arc;

/// 纯传输数据；原激活者可与当前转发者不同。
pub struct ActiveStateMessage {
    pub snapshot_hash: String,
    pub entry_id: String,
    pub activated_at_ms: i64,
    pub activated_by: String,
}

pub(super) struct CloseOnDrop(pub Arc<dyn PeerConnection>);
impl Drop for CloseOnDrop {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// 线上响应的业务投影；编码与长度上限仍由唯一 codec 负责。
pub enum ActivePullResponse {
    Envelope(Vec<u8>),
    NotAvailable,
    Locked,
    Internal,
}
