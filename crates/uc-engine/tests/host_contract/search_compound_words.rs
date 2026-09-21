use std::time::Duration;

use uc_engine::{
    CreateSpaceInput, Engine, EngineConfig, Operation, OperationResult, SearchEntriesInput,
    SecretString, SendTextInput,
};

use super::{startup::host, MemorySecureStorage};

const PASSPHRASE: &str = "search-compound-words-test-passphrase";

async fn start_engine(root: &std::path::Path) -> Engine {
    let (engine, _events) = Engine::start(
        EngineConfig::new("2.0.0"),
        host(root, Box::new(MemorySecureStorage::default())),
    )
    .await
    .unwrap();
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("search compound words test".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    engine
}

async fn send_text(engine: &Engine, text: &str) -> String {
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

async fn search(engine: &Engine, query: &str) -> Vec<String> {
    let result = engine
        .execute(Operation::SearchEntries(SearchEntriesInput {
            query: query.into(),
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

/// 终端命令里的复合词（`gitlab-runner`）位于一段更长的正文中；
/// 用户按原样输入该词或整条命令时必须能找到它，且不误中其他条目。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compound_words_inside_longer_text_are_found_by_their_natural_spelling() {
    let root = tempfile::tempdir().unwrap();
    let engine = start_engine(root.path()).await;
    let runner = send_text(&engine, "sudo systemctl restart gitlab-runner").await;
    let _nginx = send_text(&engine, "sudo systemctl restart nginx").await;
    let _notes = send_text(&engine, "gitlab pipeline notes").await;
    let compose = send_text(&engine, "open docker-compose.yml in src/lib").await;

    assert_eq!(search(&engine, "gitlab-runner").await, [runner.clone()]);
    assert_eq!(
        search(&engine, "sudo systemctl restart gitlab-runner").await,
        [runner.clone()]
    );
    assert_eq!(search(&engine, "gitlab-run").await, [runner]);
    assert_eq!(
        search(&engine, "docker-compose.yml").await,
        [compose.clone()]
    );
    assert_eq!(search(&engine, "src/lib").await, [compose]);
    assert!(search(&engine, "gitlab-runner nginx").await.is_empty());

    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}
