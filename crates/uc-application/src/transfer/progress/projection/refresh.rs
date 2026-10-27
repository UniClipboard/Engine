//! 一个完整核对动作；调用方不拼接网络结果、逐项采用或变化发布。
use super::{ProgressProjectionKey, ProgressProjectionRegistry};
use crate::transfer::progress::{
    ProgressExchangeService, ProgressQueryResult, ProgressReceiveError,
};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// 原投影和统一交换监督共同组成一次完整核对；依赖只在组装时提供。
pub struct TransferProgressReconciler {
    projections: ProgressProjectionRegistry,
    exchanges: ProgressExchangeService,
}
impl TransferProgressReconciler {
    pub fn new(
        projections: ProgressProjectionRegistry,
        exchanges: ProgressExchangeService,
    ) -> Self {
        Self {
            projections,
            exchanges,
        }
    }
    pub async fn refresh(
        &self,
        key: ProgressProjectionKey,
        cancel: &CancellationToken,
    ) -> ProgressQueryResult {
        let Some(receiver) = self.projections.receiver(key) else {
            return Arc::new(Err(ProgressReceiveError::UnknownTask));
        };
        let (intent, target) = match receiver.query_scope() {
            Ok(scope) => scope,
            Err(error) => return Arc::new(Err(error)),
        };
        let descriptor = target.expected;
        let result = self
            .exchanges
            .read_snapshot(key.peer, target, intent, cancel)
            .await;
        // 失败不写投影，保留同一不可变结果和完整下层来源。
        if result.is_err() {
            return result;
        }
        if !self.projections.current(key, &receiver) {
            return Arc::new(Err(ProgressReceiveError::Stale));
        }
        if let Ok(reply) = result.as_ref() {
            if let Err(error) = receiver.finish_query(descriptor, reply) {
                return Arc::new(Err(error));
            }
            if !self.projections.publish(key, &receiver) {
                return Arc::new(Err(ProgressReceiveError::Stale));
            }
        }
        result
    }
}
