use std::time::Duration;

use uc_engine::{
    ClipboardRestoreMode, CreateSpaceInput, Engine, EngineConfig, Operation, OperationResult,
    RestoreClipboardInput, SearchEntriesInput, SecretString, SendTextInput,
};

use super::{startup::host, MemorySecureStorage};

const PASSPHRASE: &str = "resurface-order-test-passphrase";

async fn start_engine(root: &std::path::Path) -> Engine {
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root, Box::new(MemorySecureStorage::default())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("resurface order test".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    engine
}

/// 活跃时间以毫秒计；间隔发送，避免同一毫秒内的并列排序干扰断言。
async fn send_text(engine: &Engine, text: &str) -> String {
    tokio::time::sleep(Duration::from_millis(5)).await;
    let result = engine
        .execute(Operation::SendText(SendTextInput {
            text: text.into(),
            target_devices: Vec::new(),
        }))
        .await
        .unwrap();
    let OperationResult::EntrySent(sent) = result else {
        panic!("expected saved entry")
    };
    sent.entry_id
}

/// 无关键词的浏览查询：列表界面使用的同一条路径，按索引中的活跃时间倒序。
async fn browse_order(engine: &Engine) -> Vec<String> {
    let result = engine
        .execute(Operation::SearchEntries(SearchEntriesInput {
            query: String::new(),
            operator: None,
            time_preset: None,
            from_ms: None,
            to_ms: None,
            content_types: None,
            extensions: None,
            source_devices: None,
            tags: None,
            limit: 50,
            offset: 0,
        }))
        .await
        .unwrap();
    let OperationResult::SearchPage(page) = result else {
        panic!("expected search page")
    };
    page.items.into_iter().map(|item| item.entry_id).collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn copying_existing_content_again_moves_it_to_the_front_of_browse() {
    let root = tempfile::tempdir().unwrap();
    let engine = start_engine(root.path()).await;
    let alpha = send_text(&engine, "alpha").await;
    let beta = send_text(&engine, "beta").await;
    let gamma = send_text(&engine, "gamma").await;
    assert_eq!(
        browse_order(&engine).await,
        [gamma.clone(), beta.clone(), alpha.clone()]
    );

    let alpha_again = send_text(&engine, "alpha").await;

    assert_eq!(alpha_again, alpha, "identical content reuses the entry");
    assert_eq!(browse_order(&engine).await, [alpha, gamma, beta]);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restoring_an_entry_moves_it_to_the_front_of_browse() {
    let root = tempfile::tempdir().unwrap();
    let engine = start_engine(root.path()).await;
    let alpha = send_text(&engine, "alpha").await;
    let beta = send_text(&engine, "beta").await;
    let gamma = send_text(&engine, "gamma").await;

    tokio::time::sleep(Duration::from_millis(5)).await;
    engine
        .execute(Operation::RestoreClipboard(RestoreClipboardInput {
            entry_id: alpha.clone(),
            mode: ClipboardRestoreMode::Standard,
        }))
        .await
        .unwrap();

    assert_eq!(browse_order(&engine).await, [alpha, gamma, beta]);
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}
