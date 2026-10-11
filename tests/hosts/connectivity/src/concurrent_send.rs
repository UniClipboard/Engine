//! 通过稳定 Engine 入口同时发起四次独立发送，不添加生产流程开关。
use anyhow::{bail, Result};
use serde_json::{json, Value};
use uc_engine::{Engine, Operation, OperationResult, SendTextInput};

pub async fn send(engine: &Engine, request: &Value) -> Result<Value> {
    let Some(peer) = request["peer"].as_str() else {
        bail!("missing peer");
    };
    let Some(texts) = request["texts"].as_array() else {
        bail!("missing texts");
    };
    if texts.len() != 4 {
        bail!("exactly four texts required");
    }
    let mut inputs = Vec::new();
    for text in texts {
        let Some(text) = text.as_str() else {
            bail!("invalid text");
        };
        inputs.push(SendTextInput {
            text: text.into(),
            target_devices: vec![peer.into()],
        });
    }
    let mut inputs = inputs.into_iter();
    let (Some(a), Some(b), Some(c), Some(d)) =
        (inputs.next(), inputs.next(), inputs.next(), inputs.next())
    else {
        bail!("missing send inputs");
    };
    let results = tokio::join!(
        engine.execute(Operation::SendText(a)),
        engine.execute(Operation::SendText(b)),
        engine.execute(Operation::SendText(c)),
        engine.execute(Operation::SendText(d))
    );
    let mut reports = Vec::new();
    for result in [results.0, results.1, results.2, results.3] {
        let OperationResult::EntrySent(report) = result? else {
            bail!("send report expected");
        };
        reports.push(serde_json::to_value(report)?);
    }
    Ok(json!(reports))
}
