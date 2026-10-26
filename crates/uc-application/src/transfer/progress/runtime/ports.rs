use super::super::{
    ProgressInboundWirePort, ProgressReceiverPort, ProgressSnapshotProviderPort,
    ProgressSnapshotWirePort, ProgressUnattributedReceiverPort, ProgressWirePort,
};
use crate::connectivity::{ConnectivityBackend, ElapsedClockPort, SessionConnectionAdmissionPort};
use std::sync::Arc;

/// 组装层只提供协议实现工厂；投影与实际接收权威仍由Application完整运行期选择。
pub type ProgressInboundFactory = Box<
    dyn FnOnce(
            Arc<dyn ProgressReceiverPort>,
            Arc<dyn ProgressUnattributedReceiverPort>,
            Arc<dyn ProgressSnapshotProviderPort>,
        ) -> Arc<dyn ProgressInboundWirePort>
        + Send,
>;

/// 同一活动会话的进度网络能力；不允许为测试隐式补空依赖或另建连接缓存。
pub struct TransferProgressNetwork {
    pub backend: Arc<dyn ConnectivityBackend>,
    pub admission: Arc<dyn SessionConnectionAdmissionPort>,
    pub wire: Arc<dyn ProgressWirePort>,
    pub snapshot_wire: Arc<dyn ProgressSnapshotWirePort>,
    pub inbound: ProgressInboundFactory,
    pub clock: Arc<dyn ElapsedClockPort>,
}
