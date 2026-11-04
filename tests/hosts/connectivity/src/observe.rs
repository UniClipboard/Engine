//! 规范化可观察状态：只经公开 Engine 操作读取，输出与版本无关的稳定字段和内容摘要，
//! 供同一单元在升级、降级前后比较。输出只含测试合成数据的摘要，不含原文。

use anyhow::{bail, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uc_engine::{
    BlobResourceInput, Engine, HistoryEntryInput, ListHistoryEntriesInput, Operation,
    OperationResult,
};

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn query(engine: &Engine, operation: Operation) -> Result<OperationResult, Value> {
    engine
        .execute(operation)
        .await
        .map_err(|error| json!({ "error": "operation_failed", "code": error.code() }))
}

async fn read_file(engine: &Engine, entry: HistoryEntryInput) -> Result<Value, Value> {
    match query(engine, Operation::ReadEntryFile(entry)).await? {
        OperationResult::EntryFileRead(file) => Ok(json!({
            "bytes": digest(&file.bytes),
            "size": file.bytes.len(),
            "name": digest(file.file_name.as_bytes()),
        })),
        _ => Err(json!({ "error": "unexpected_result" })),
    }
}

async fn read_resource(engine: &Engine, entry: HistoryEntryInput) -> Result<Value, Value> {
    let OperationResult::HistoryEntryResource(resource) =
        query(engine, Operation::GetHistoryEntryResource(entry)).await?
    else {
        return Err(json!({ "error": "unexpected_result" }));
    };
    let bytes = match (resource.inline_data, resource.blob_id) {
        (Some(bytes), _) => bytes,
        (None, Some(blob_id)) => {
            match query(engine, Operation::ReadBlob(BlobResourceInput { blob_id })).await? {
                OperationResult::BlobRead(blob) => blob.bytes,
                _ => return Err(json!({ "error": "unexpected_result" })),
            }
        }
        (None, None) => return Err(json!({ "error": "resource_missing" })),
    };
    Ok(json!({ "bytes": digest(&bytes), "size": bytes.len() }))
}

async fn read_detail(engine: &Engine, entry: HistoryEntryInput) -> Result<Value, Value> {
    match query(engine, Operation::GetHistoryEntry(entry)).await? {
        OperationResult::HistoryEntry(detail) => Ok(json!({
            "bytes": digest(detail.content.as_bytes()),
            "size": detail.content.len(),
        })),
        _ => Err(json!({ "error": "unexpected_result" })),
    }
}

/// 读取条目的实际内容并摘要：按内容类型先用对应读取方式，失败再依次尝试其余公开读取；
/// 全部失败时如实记录每种方式的错误码，不把失败当成空内容。
async fn entry_content(engine: &Engine, entry_id: &str, content_type: &str) -> Value {
    let entry = || HistoryEntryInput {
        entry_id: entry_id.to_owned(),
    };
    let order: [&str; 3] = match content_type {
        "text" => ["detail", "file", "resource"],
        "image" => ["resource", "file", "detail"],
        _ => ["file", "resource", "detail"],
    };
    let mut errors = serde_json::Map::new();
    for method in order {
        let result = match method {
            "file" => read_file(engine, entry()).await,
            "resource" => read_resource(engine, entry()).await,
            _ => read_detail(engine, entry()).await,
        };
        match result {
            Ok(content) => return content,
            Err(error) => {
                errors.insert(method.to_owned(), error);
            }
        }
    }
    json!({ "read_failed": errors })
}

async fn history(engine: &Engine) -> Result<Value> {
    let OperationResult::HistoryEntries(entries) = engine
        .execute(Operation::ListHistoryEntries(ListHistoryEntriesInput {
            limit: 100,
            offset: 0,
        }))
        .await?
    else {
        bail!("history entries expected")
    };
    let mut observed = Vec::with_capacity(entries.len());
    for entry in &entries {
        let content = entry_content(engine, &entry.entry_id, &entry.content_type).await;
        observed.push(json!({ "content_type": entry.content_type, "content": content }));
    }
    // 条目顺序依赖捕获时间与实现排序，比较时只关心集合。
    observed.sort_by_key(Value::to_string);
    Ok(Value::Array(observed))
}

async fn devices(engine: &Engine) -> Result<Value> {
    let OperationResult::Devices(devices) = engine.execute(Operation::ListDevices).await? else {
        bail!("devices expected")
    };
    let mut names = devices
        .iter()
        .map(|device| json!({ "name": device.display_name, "local": device.is_local }))
        .collect::<Vec<_>>();
    names.sort_by_key(Value::to_string);
    Ok(Value::Array(names))
}

/// 汇总可比较状态。各项独立读取，单项失败记录错误码而不丢弃其余结果。
pub(crate) async fn observe(engine: &Engine) -> Result<Value> {
    let setup = match query(engine, Operation::QuerySetupState).await {
        Ok(OperationResult::SetupState(state)) => {
            json!({ "has_completed": state.has_completed, "has_space": state.space_id.is_some() })
        }
        Ok(_) => json!({ "error": "unexpected_result" }),
        Err(error) => error,
    };
    let encryption = match query(engine, Operation::QueryEncryptionState).await {
        Ok(OperationResult::EncryptionState(state)) => serde_json::to_value(state)?,
        Ok(_) => json!({ "error": "unexpected_result" }),
        Err(error) => error,
    };
    let local_device = match query(engine, Operation::QueryLocalDevice).await {
        Ok(OperationResult::LocalDevice(device)) => {
            json!({ "name": device.display_name, "id": device.device_id })
        }
        Ok(_) => json!({ "error": "unexpected_result" }),
        Err(error) => error,
    };
    let settings = match query(engine, Operation::QuerySettings).await {
        Ok(OperationResult::Settings(settings)) => json!({
            "device_name": settings.general.device_name,
            "sync_on_restore": settings.sync.sync_on_restore,
        }),
        Ok(_) => json!({ "error": "unexpected_result" }),
        Err(error) => error,
    };
    let upgrade = match query(engine, Operation::QueryUpgradeStatus).await {
        Ok(OperationResult::UpgradeStatus(status)) => serde_json::to_value(status)?,
        Ok(_) => json!({ "error": "unexpected_result" }),
        Err(error) => error,
    };
    let devices = devices(engine)
        .await
        .unwrap_or_else(|error| json!({ "error": "operation_failed", "code": error_code(&error) }));
    let history = history(engine)
        .await
        .unwrap_or_else(|error| json!({ "error": "operation_failed", "code": error_code(&error) }));
    Ok(json!({
        "setup": setup,
        "encryption": encryption,
        "local_device": local_device,
        "devices": devices,
        "settings": settings,
        "upgrade": upgrade,
        "history": history,
    }))
}

fn error_code(error: &anyhow::Error) -> Option<u32> {
    error
        .downcast_ref::<uc_engine::EngineError>()
        .map(uc_engine::EngineError::code)
}

/// 基准只读取一页真实内容，不返回内容正文或设备信息。
pub(crate) async fn text_page(engine: &Engine, request: &Value) -> Result<Value> {
    let offset = request["offset"].as_u64().unwrap_or(0);
    let limit = request["limit"].as_u64().unwrap_or(1);
    if limit == 0 || limit > 32 || offset > 10000 {
        bail!("invalid benchmark page");
    }
    let OperationResult::HistoryEntries(entries) = engine
        .execute(Operation::ListHistoryEntries(ListHistoryEntriesInput {
            limit: limit as u32,
            offset,
        }))
        .await?
    else {
        bail!("history entries expected");
    };
    let mut result = Vec::new();
    for entry in entries {
        let OperationResult::HistoryEntry(detail) = engine
            .execute(Operation::GetHistoryEntry(HistoryEntryInput {
                entry_id: entry.entry_id,
            }))
            .await?
        else {
            bail!("history detail expected");
        };
        result.push(
            json!({"sha256": digest(detail.content.as_bytes()), "size": detail.content.len()}),
        );
    }
    Ok(json!(result))
}
