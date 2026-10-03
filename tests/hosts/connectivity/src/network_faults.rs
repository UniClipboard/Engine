//! 测试宿主的连接故障控制；只复用 Engine 既有 dev-tools 能力。

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use uc_engine::{DevOperation, DevOperationResult, Engine};

pub async fn execute(engine: &Engine, command: &str, request: &Value) -> Result<Value> {
    match command {
        "network_endpoint" => {
            let DevOperationResult::NetworkEndpointId(endpoint_id) = engine
                .execute_dev(DevOperation::QueryNetworkEndpointId)
                .await?
            else {
                bail!("network endpoint expected");
            };
            Ok(json!(endpoint_id))
        }
        "reject_reachability_dials" => {
            let endpoint_ids = serde_json::from_value(request["endpoint_ids"].clone())
                .context("invalid endpoint identifiers")?;
            let DevOperationResult::NetworkPartitionUpdated { blocked_peer_count } = engine
                .execute_dev(DevOperation::RejectNewConnections {
                    endpoint_ids,
                    peer_reachability: true,
                })
                .await?
            else {
                bail!("connection rejection result expected");
            };
            Ok(json!({ "blocked_peer_count": blocked_peer_count }))
        }
        "rejected_dials" => {
            let DevOperationResult::RejectedConnectionCount { count } = engine
                .execute_dev(DevOperation::QueryRejectedConnectionCount)
                .await?
            else {
                bail!("rejected connection count expected");
            };
            Ok(json!(count))
        }
        _ => bail!("unknown network fault command"),
    }
}
