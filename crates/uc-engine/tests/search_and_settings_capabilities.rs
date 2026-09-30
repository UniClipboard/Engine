//! 端到端：真实 Engine、真实 SQLite 与真实加密会话下，验证桌面快捷面板依赖的查询与通知能力。
//!
//! 覆盖：标签“且/或”、与搜索同语义的批量计数、按日统计、
//! 会话锁定后的失败关闭、解锁回退与重启，以及设置变更事件。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use uc_engine::error_codes::{
    SEARCH_BAD_REQUEST_CODE, SEARCH_INDEX_NOT_READY_CODE, SEARCH_INDEX_REBUILDING_CODE,
    SEARCH_SESSION_LOCKED_CODE,
};
use uc_engine::{
    CountSearchEntriesInput, CreateSpaceInput, DailyEntryCountsInput, Engine, EngineConfig,
    EngineEvent, EventStream, GeneralSettingsPatch, HostCapabilities, HostCapabilityError,
    HostClipboard, HostClipboardSnapshot, HostDirectories, HostFileAccess, HostFileHandle,
    HostFileMetadata, HostSecureStorage, Operation, OperationResult, QuickPanelPositionSummary,
    QuickPanelSettingsPatch, SearchEntriesInput, SecretString, SendTextInput,
    SetHistoryEntryFavoriteInput, SettingsPatch, SettingsSectionSummary, UnlockSpaceInput,
};

const PASSPHRASE: &str = "quick-panel-capability-passphrase";

#[derive(Clone, Default)]
struct MemorySecureStorage(Arc<Mutex<HashMap<String, Vec<u8>>>>);

impl HostSecureStorage for MemorySecureStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, HostCapabilityError> {
        Ok(self.0.lock().unwrap().get(key).cloned())
    }

    fn set(&self, key: &str, value: &[u8]) -> Result<(), HostCapabilityError> {
        self.0
            .lock()
            .unwrap()
            .insert(key.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), HostCapabilityError> {
        self.0.lock().unwrap().remove(key);
        Ok(())
    }
}

struct EmptyClipboard;

impl HostClipboard for EmptyClipboard {
    fn read(&self) -> Result<HostClipboardSnapshot, HostCapabilityError> {
        Ok(HostClipboardSnapshot {
            observed_at_ms: 1,
            representations: Vec::new(),
        })
    }

    fn write(&self, _snapshot: HostClipboardSnapshot) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

struct EmptyFiles;

impl HostFileAccess for EmptyFiles {
    fn metadata(&self, _handle: &HostFileHandle) -> Result<HostFileMetadata, HostCapabilityError> {
        Ok(HostFileMetadata {
            display_name: "unused".into(),
            size_bytes: 0,
            mime_type: None,
        })
    }

    fn read_chunk(
        &self,
        _handle: &HostFileHandle,
        _offset: u64,
        _max_bytes: u32,
    ) -> Result<Vec<u8>, HostCapabilityError> {
        Ok(Vec::new())
    }

    fn write_chunk(
        &self,
        _handle: &HostFileHandle,
        _offset: u64,
        _bytes: &[u8],
    ) -> Result<(), HostCapabilityError> {
        Ok(())
    }

    fn finish_write(&self, _handle: &HostFileHandle) -> Result<(), HostCapabilityError> {
        Ok(())
    }
}

fn host(root: &Path, storage: &MemorySecureStorage) -> HostCapabilities {
    HostCapabilities::new(
        HostDirectories::new(
            root.join("private"),
            root.join("cache"),
            root.join("temporary"),
            root.join("logs"),
        ),
        Box::new(storage.clone()),
        Box::new(EmptyClipboard),
        Box::new(EmptyFiles),
    )
}

async fn start(root: &Path, storage: &MemorySecureStorage) -> (Engine, EventStream) {
    Engine::start(EngineConfig::new("2.0.0"), host(root, storage))
        .await
        .expect("engine starts")
}

async fn create_space(engine: &Engine) {
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("panel capability test".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect("space is created");
}

async fn send_text(engine: &Engine, text: &str) -> String {
    let OperationResult::EntrySent(sent) = engine
        .execute(Operation::SendText(SendTextInput {
            text: text.into(),
            target_devices: Vec::new(),
        }))
        .await
        .expect("text is saved")
    else {
        panic!("expected a saved entry")
    };
    sent.entry_id
}

fn filters() -> SearchEntriesInput {
    SearchEntriesInput {
        query: String::new(),
        operator: None,
        time_preset: None,
        from_ms: None,
        to_ms: None,
        content_types: None,
        extensions: None,
        source_devices: None,
        tags: None,
        tag_match: None,
        limit: 50,
        offset: 0,
    }
}

fn tagged(tags: &str, mode: Option<&str>) -> SearchEntriesInput {
    SearchEntriesInput {
        tags: Some(tags.into()),
        tag_match: mode.map(str::to_owned),
        ..filters()
    }
}

async fn search(engine: &Engine, input: SearchEntriesInput) -> uc_engine::SearchPageSummary {
    match engine.execute(Operation::SearchEntries(input)).await {
        Ok(OperationResult::SearchPage(page)) => page,
        other => panic!("unexpected search result: {other:?}"),
    }
}

/// 解锁或重启后索引会重建；重建期间计数按设计返回“重建中”，轮询到可用为止。
async fn counts(engine: &Engine, queries: Vec<SearchEntriesInput>) -> Vec<u32> {
    let mut last = None;
    for _ in 0..100 {
        match engine
            .execute(Operation::CountSearchEntries(CountSearchEntriesInput {
                queries: queries.clone(),
            }))
            .await
        {
            Ok(OperationResult::SearchCounts(counts)) => return counts,
            Err(error)
                if error.code() == SEARCH_INDEX_REBUILDING_CODE
                    || error.code() == SEARCH_INDEX_NOT_READY_CODE =>
            {
                last = Some(error.code());
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => panic!("unexpected count result: {other:?}"),
        }
    }
    panic!("index stayed unavailable for counts: {last:?}");
}

async fn daily(engine: &Engine, boundaries_ms: Vec<i64>) -> Result<Vec<u32>, u32> {
    match engine
        .execute(Operation::QueryDailyEntryCounts(DailyEntryCountsInput {
            boundaries_ms,
        }))
        .await
    {
        Ok(OperationResult::DailyEntryCounts(counts)) => Ok(counts),
        Ok(other) => panic!("unexpected daily result: {other:?}"),
        Err(error) => Err(error.code()),
    }
}

/// 索引在保存后异步追上；轮询直到条目数达到预期。
async fn wait_for_indexed(engine: &Engine, expected: u32) {
    for _ in 0..100 {
        if search(engine, filters()).await.total == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("index did not reach {expected} entries");
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test(flavor = "multi_thread")]
async fn panel_queries_share_one_semantics_and_fail_closed_while_locked() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, _events) = start(root.path(), &storage).await;
    create_space(&engine).await;

    let _docs = send_text(&engine, "https://example.com/docs").await;
    let site = send_text(&engine, "https://rust-lang.org").await;
    let code = send_text(
        &engine,
        "fn main() -> Result<(), String> {\n    let value = 1;\n    Ok(())\n}",
    )
    .await;
    let _color = send_text(&engine, "#ff8800").await;
    let _json = send_text(&engine, "{\"name\": \"uni\", \"items\": [1, 2, 3]}").await;
    let _note = send_text(&engine, "a plain note without structure").await;
    wait_for_indexed(&engine, 6).await;
    for entry_id in [&site, &code] {
        engine
            .execute(Operation::SetHistoryEntryFavorite(
                SetHistoryEntryFavoriteInput {
                    entry_id: entry_id.clone(),
                    is_favorited: true,
                },
            ))
            .await
            .unwrap();
    }

    // U3：默认取“或”，`all` 要求同时携带；未知取值被拒绝。
    let any = search(&engine, tagged("link,favorited", None)).await;
    assert_eq!(any.total, 3, "link 两条 + 仅收藏的代码一条");
    let all = search(&engine, tagged("link,favorited", Some("all"))).await;
    assert_eq!(all.total, 1);
    assert_eq!(all.items[0].entry_id, site);
    assert_eq!(
        search(&engine, tagged("link,link", Some("all")))
            .await
            .total,
        2
    );
    let rejected = engine
        .execute(Operation::SearchEntries(tagged("link", Some("either"))))
        .await
        .unwrap_err();
    assert_eq!(rejected.code(), SEARCH_BAD_REQUEST_CODE);

    // U4：批量计数与搜索 total 同源，按输入顺序返回，分页字段被忽略。
    let queries = vec![
        tagged("link,favorited", None),
        tagged("link,favorited", Some("all")),
        filters(),
        SearchEntriesInput {
            content_types: Some("text".into()),
            limit: 1,
            offset: 3,
            ..filters()
        },
        SearchEntriesInput {
            query: "plain".into(),
            ..filters()
        },
    ];
    let mut expected = Vec::new();
    for query in &queries {
        expected.push(search(&engine, query.clone()).await.total);
    }
    assert_eq!(counts(&engine, queries.clone()).await, expected);
    assert_eq!(expected[0], 3);
    assert_eq!(expected[1], 1);
    assert_eq!(expected[2], 6);
    assert_eq!(expected[4], 1);
    assert_eq!(counts(&engine, Vec::new()).await, Vec::<u32>::new());
    let too_many = vec![filters(); 33];
    let error = engine
        .execute(Operation::CountSearchEntries(CountSearchEntriesInput {
            queries: too_many,
        }))
        .await
        .unwrap_err();
    assert_eq!(error.code(), SEARCH_BAD_REQUEST_CODE);

    // U5：按调用方给出的本地日边界分桶。
    let now = now_ms();
    let day = 86_400_000;
    assert_eq!(
        daily(&engine, vec![now - 2 * day, now - day, now + day]).await,
        Ok(vec![0, 6])
    );
    assert_eq!(
        daily(&engine, vec![now + day, now + 2 * day]).await,
        Ok(vec![0])
    );
    for invalid in [vec![], vec![1], vec![5, 5], vec![9, 3]] {
        assert_eq!(daily(&engine, invalid).await, Err(SEARCH_BAD_REQUEST_CODE));
    }
    assert_eq!(
        daily(&engine, (0..=401).collect()).await,
        Err(SEARCH_BAD_REQUEST_CODE),
        "桶数超过上限"
    );

    // 锁定后：计数与按日统计在 Engine 内部失败关闭，不泄露数量。
    // （既有的 SearchEntries 在用户锁定后仍由后台持有的密钥解密，前置的会话检查由宿主负责；
    // 见交接文档中的已知差异。）
    engine.execute(Operation::LockEncryption).await.unwrap();
    for locked in [
        engine
            .execute(Operation::CountSearchEntries(CountSearchEntriesInput {
                queries: vec![filters(), tagged("link", Some("all"))],
            }))
            .await
            .map(|_| ()),
        engine
            .execute(Operation::QueryDailyEntryCounts(DailyEntryCountsInput {
                boundaries_ms: vec![now - day, now + day],
            }))
            .await
            .map(|_| ()),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(locked.1.unwrap_err().code(), SEARCH_SESSION_LOCKED_CODE);
    }

    // 错误口令不解锁；正确口令解锁后结果与锁定前一致。
    assert!(engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new("definitely-not-the-passphrase"),
        }))
        .await
        .is_err());
    assert_eq!(
        engine
            .execute(Operation::CountSearchEntries(CountSearchEntriesInput {
                queries: vec![filters()],
            }))
            .await
            .unwrap_err()
            .code(),
        SEARCH_SESSION_LOCKED_CODE
    );
    engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .unwrap();
    assert_eq!(counts(&engine, queries).await, expected);
    assert_eq!(
        daily(&engine, vec![now - 2 * day, now - day, now + day]).await,
        Ok(vec![0, 6])
    );

    // 重启后同一份加密数据仍可计数（口令解锁后）。
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let (restarted, _events) = start(root.path(), &storage).await;
    let _ = restarted
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await;
    wait_for_indexed(&restarted, 6).await;
    assert_eq!(
        counts(
            &restarted,
            vec![tagged("link,favorited", Some("all")), filters()]
        )
        .await,
        vec![1, 6]
    );
    restarted.shutdown(Duration::from_secs(15)).await.unwrap();
}

async fn next_settings_event(events: &mut EventStream) -> Option<Vec<SettingsSectionSummary>> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, events.next()).await {
            Ok(Some(EngineEvent::SettingsChanged(changed))) => return Some(changed.sections),
            Ok(Some(_)) => continue,
            Ok(None) | Err(_) => return None,
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn settings_updates_emit_a_section_only_change_event() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, mut events) = start(root.path(), &storage).await;
    create_space(&engine).await;
    // 建立空间时写入的设备名也是一次设置保存，先取走这条通知。
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(500), events.next()).await {}

    let update = |patch: SettingsPatch| {
        let engine = &engine;
        async move {
            engine
                .execute(Operation::UpdateSettings(Box::new(patch)))
                .await
                .expect("settings update succeeds")
        }
    };

    update(SettingsPatch {
        general: Some(GeneralSettingsPatch {
            language: Some(Some("zh-CN".into())),
            ..Default::default()
        }),
        quick_panel: Some(QuickPanelSettingsPatch {
            position: Some(QuickPanelPositionSummary::FollowCursor),
            ..Default::default()
        }),
        keyboard_shortcuts: Some(
            [(
                "quick_panel.toggle".to_string(),
                Some(uc_engine::ShortcutKeySummary::Single("ctrl+alt+p".into())),
            )]
            .into(),
        ),
        ..Default::default()
    })
    .await;
    let sections = next_settings_event(&mut events)
        .await
        .expect("a successful update emits SettingsChanged");
    for expected in [
        SettingsSectionSummary::General,
        SettingsSectionSummary::QuickPanel,
        SettingsSectionSummary::KeyboardShortcuts,
    ] {
        assert!(sections.contains(&expected), "{expected:?} in {sections:?}");
    }
    assert!(!sections.contains(&SettingsSectionSummary::Network));

    // 内容没变的保存不通知。
    update(SettingsPatch {
        general: Some(GeneralSettingsPatch {
            language: Some(Some("zh-CN".into())),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), next_settings_event(&mut events))
            .await
            .ok()
            .flatten(),
        None,
        "identical save must stay silent"
    );

    // 被拒绝的更新不产生事件，设置保持原样。
    let rejected = update(SettingsPatch {
        network: Some(uc_engine::NetworkSettingsPatch {
            custom_relay_urls: Some(vec!["ftp://not-a-relay.invalid".into()]),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await;
    assert!(matches!(
        rejected,
        OperationResult::SettingsUpdated(uc_engine::SettingsUpdateOutcome::Rejected { .. })
    ));
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), next_settings_event(&mut events))
            .await
            .ok()
            .flatten(),
        None,
        "rejected update must stay silent"
    );
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}

/// 并发更新不同分区：无论谁先落盘，最终设置里的每处变化都必须被某条通知覆盖。
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_settings_updates_are_all_covered_by_notifications() {
    let root = tempfile::tempdir().unwrap();
    let storage = MemorySecureStorage::default();
    let (engine, mut events) = start(root.path(), &storage).await;
    create_space(&engine).await;
    while let Ok(Some(_)) = tokio::time::timeout(Duration::from_millis(500), events.next()).await {}

    let engine = Arc::new(engine);
    let mut tasks = Vec::new();
    for round in 0..8 {
        let language = format!("lang-{round}");
        let engine_a = Arc::clone(&engine);
        let engine_b = Arc::clone(&engine);
        tasks.push(tokio::spawn(async move {
            engine_a
                .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
                    general: Some(GeneralSettingsPatch {
                        language: Some(Some(language)),
                        ..Default::default()
                    }),
                    ..Default::default()
                })))
                .await
                .unwrap();
        }));
        tasks.push(tokio::spawn(async move {
            engine_b
                .execute(Operation::UpdateSettings(Box::new(SettingsPatch {
                    quick_panel: Some(QuickPanelSettingsPatch {
                        enabled: Some(round % 2 == 0),
                        ..Default::default()
                    }),
                    ..Default::default()
                })))
                .await
                .unwrap();
        }));
    }
    for task in tasks {
        task.await.unwrap();
    }

    let mut seen = std::collections::HashSet::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(1), events.next()).await {
        if let EngineEvent::SettingsChanged(changed) = event {
            seen.extend(changed.sections);
        }
    }
    assert!(seen.contains(&SettingsSectionSummary::General), "{seen:?}");
    assert!(
        seen.contains(&SettingsSectionSummary::QuickPanel),
        "{seen:?}"
    );
    let OperationResult::Settings(final_settings) =
        engine.execute(Operation::QuerySettings).await.unwrap()
    else {
        panic!("expected settings")
    };
    // 并发任务的落盘顺序不固定，只断言最终值确实来自这些更新之一。
    assert!(final_settings
        .general
        .language
        .as_deref()
        .is_some_and(|language| language.starts_with("lang-")));
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
}
