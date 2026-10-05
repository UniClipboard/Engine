//! 端到端：真实 Engine、真实 SQLite 与真实加密会话下的本机历史标签生命周期。
//!
//! 覆盖：名称规范化与同名幂等创建、批量关联/移除（缺失条目跳过）、按标签搜索、
//! 计数与标签列表、多条目汇总、改名冲突、合并去重、删除标签保留内容、条目删除级联、
//! 会话锁定失败关闭、重启与显式索引重建后标签仍可查询，以及名称与关联不以明文落盘
//! （权威关联只存密文，派生索引只存 HMAC 词项）。
//!
//! 设置 `UC_HISTORY_TAGS_EVIDENCE_DIR` 时，Engine 资料目录与证据 JSON 写入该目录，
//! 便于复核数据库摘要与内容哈希。

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use diesel::sql_types::BigInt;
use diesel::{Connection, QueryableByName, RunQueryDsl, SqliteConnection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uc_engine::error_codes::{
    HISTORY_FAILED_CODE, HISTORY_INVALID_INPUT_CODE, HISTORY_NOT_FOUND_CODE,
    HISTORY_TAGS_LOCKED_CODE, SEARCH_INDEX_NOT_READY_CODE, SEARCH_INDEX_REBUILDING_CODE,
};
use uc_engine::{
    CountSearchEntriesInput, CreateHistoryTagInput, CreateSpaceInput, Engine, EngineConfig,
    EngineError, EventStream, HistoryEntryInput, HistoryEntryTagsInput, HistoryTagEntriesInput,
    HistoryTagInput, HistoryTagRenameSummary, HistoryTagSummary, HostCapabilities,
    HostCapabilityError, HostClipboard, HostClipboardSnapshot, HostDirectories, HostFileAccess,
    HostFileHandle, HostFileMetadata, HostSecureStorage, MergeHistoryTagsInput, Operation,
    OperationResult, RenameHistoryTagInput, SearchEntriesInput, SecretString, SendTextInput,
    UnlockSpaceInput,
};

const PASSPHRASE: &str = "history-tags-lifecycle-passphrase";
/// 有辨识度的名称，便于在资料目录中确认它们从未以明文出现。
const WORK: &str = "Zeta-Projekt-91";
const PERSONAL: &str = "Kappa-Ordner-42";
const ARCHIVE: &str = "Lambda-Archiv-17";
const CAFE_DECOMPOSED: &str = "  Mu-Cafe\u{301}-08 ";
const CAFE: &str = "Mu-Café-08";
const RENAMED_WORK: &str = "ZETA-PROJEKT-91";

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

async fn unlock(engine: &Engine) {
    engine
        .execute(Operation::UnlockSpace(UnlockSpaceInput {
            passphrase: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect("space unlocks");
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

fn tagged(tags: &[&str], mode: Option<&str>) -> SearchEntriesInput {
    SearchEntriesInput {
        tags: Some(tags.join(",")),
        tag_match: mode.map(str::to_owned),
        ..filters()
    }
}

/// 解锁、重启或重建后索引可能暂不可用；轮询到可用为止。
async fn search(engine: &Engine, input: SearchEntriesInput) -> uc_engine::SearchPageSummary {
    for _ in 0..150 {
        match engine
            .execute(Operation::SearchEntries(input.clone()))
            .await
        {
            Ok(OperationResult::SearchPage(page)) if page.state == "ready" => return page,
            Ok(OperationResult::SearchPage(_)) => {}
            Err(error)
                if error.code() == SEARCH_INDEX_REBUILDING_CODE
                    || error.code() == SEARCH_INDEX_NOT_READY_CODE => {}
            other => panic!("unexpected search result: {other:?}"),
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("search index stayed unavailable");
}

async fn entry_ids(engine: &Engine, input: SearchEntriesInput) -> Vec<String> {
    let mut ids: Vec<String> = search(engine, input)
        .await
        .items
        .into_iter()
        .map(|item| item.entry_id)
        .collect();
    ids.sort();
    ids
}

async fn wait_for_indexed(engine: &Engine, expected: u32) {
    for _ in 0..150 {
        if search(engine, filters()).await.total == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("index did not reach {expected} entries");
}

async fn count(engine: &Engine, input: SearchEntriesInput) -> u32 {
    for _ in 0..150 {
        match engine
            .execute(Operation::CountSearchEntries(CountSearchEntriesInput {
                queries: vec![input.clone()],
            }))
            .await
        {
            Ok(OperationResult::SearchCounts(counts)) => return counts[0],
            Err(error)
                if error.code() == SEARCH_INDEX_REBUILDING_CODE
                    || error.code() == SEARCH_INDEX_NOT_READY_CODE =>
            {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            other => panic!("unexpected count result: {other:?}"),
        }
    }
    panic!("index stayed unavailable for counts");
}

async fn search_tag_count(engine: &Engine, tag_id: &str) -> Option<(u32, bool)> {
    match engine.execute(Operation::QuerySearchTags).await {
        Ok(OperationResult::SearchTags(tags)) => tags
            .into_iter()
            .find(|tag| tag.tag_id == tag_id)
            .map(|tag| (tag.count, tag.is_builtin)),
        other => panic!("unexpected search tags result: {other:?}"),
    }
}

async fn list_tags(engine: &Engine) -> Vec<HistoryTagSummary> {
    match engine.execute(Operation::ListHistoryTags).await {
        Ok(OperationResult::HistoryTags(tags)) => tags,
        other => panic!("unexpected tag list: {other:?}"),
    }
}

async fn create(engine: &Engine, name: &str) -> Result<(HistoryTagSummary, bool), EngineError> {
    match engine
        .execute(Operation::CreateHistoryTag(CreateHistoryTagInput {
            name: name.into(),
        }))
        .await?
    {
        OperationResult::HistoryTagCreated(created) => Ok((created.tag, created.created)),
        other => panic!("unexpected create result: {other:?}"),
    }
}

async fn change_entries(
    engine: &Engine,
    add: bool,
    tag_id: &str,
    entry_ids: &[&str],
) -> Result<uc_engine::HistoryTagBatchSummary, EngineError> {
    let input = HistoryTagEntriesInput {
        tag_id: tag_id.into(),
        entry_ids: entry_ids.iter().map(|id| id.to_string()).collect(),
    };
    let operation = if add {
        Operation::AddHistoryTagToEntries(input)
    } else {
        Operation::RemoveHistoryTagFromEntries(input)
    };
    match engine.execute(operation).await? {
        OperationResult::HistoryTagEntriesChanged(batch) => Ok(batch),
        other => panic!("unexpected batch result: {other:?}"),
    }
}

async fn content_hash(engine: &Engine, entry_id: &str) -> String {
    match engine
        .execute(Operation::GetHistoryEntry(HistoryEntryInput {
            entry_id: entry_id.into(),
        }))
        .await
    {
        Ok(OperationResult::HistoryEntry(detail)) => {
            hex(&Sha256::digest(detail.content.as_bytes()))
        }
        other => panic!("unexpected entry detail: {other:?}"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn tag_by_id<'a>(tags: &'a [HistoryTagSummary], tag_id: &str) -> Option<&'a HistoryTagSummary> {
    tags.iter().find(|tag| tag.tag_id == tag_id)
}

fn code<T: std::fmt::Debug>(result: Result<T, EngineError>) -> u32 {
    result.expect_err("operation must fail").code()
}

fn files_under(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

#[derive(QueryableByName)]
struct Count {
    #[diesel(sql_type = BigInt)]
    n: i64,
}

fn scalar(conn: &mut SqliteConnection, sql: &str) -> i64 {
    diesel::sql_query(sql)
        .get_result::<Count>(conn)
        .expect("count query")
        .n
}

/// 找到正在使用的 profile 数据库（资料目录中可能还有升级暂存副本，按条目数选择）。
fn profile_database(root: &Path) -> PathBuf {
    files_under(root)
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == "profile.sqlite")
        })
        .filter_map(|path| {
            let mut conn = SqliteConnection::establish(&path.to_string_lossy()).ok()?;
            let has_table = scalar(
                &mut conn,
                "SELECT COUNT(*) AS n FROM sqlite_master WHERE name = 'history_tag'",
            ) == 1;
            has_table.then(|| {
                let entries = scalar(&mut conn, "SELECT COUNT(*) AS n FROM clipboard_entry");
                (entries, path)
            })
        })
        .max_by_key(|(entries, _)| *entries)
        .map(|(_, path)| path)
        .expect("profile database with history_tag exists")
}

/// 汇总权威表与派生索引中的标签行。
fn database_summary(root: &Path) -> Value {
    let database = profile_database(root);
    let mut conn =
        SqliteConnection::establish(&database.to_string_lossy()).expect("open profile database");
    json!({
        "database": database.strip_prefix(root).unwrap_or(&database).to_string_lossy(),
        "clipboard_entry_rows": scalar(&mut conn, "SELECT COUNT(*) AS n FROM clipboard_entry"),
        "history_tag_rows": scalar(&mut conn, "SELECT COUNT(*) AS n FROM history_tag"),
        "history_tag_assignment_rows": scalar(
            &mut conn,
            "SELECT COUNT(*) AS n FROM history_tag_assignment"
        ),
        // 以下三项必须为 0：明文标签 id 不出现在派生标签表、posting 词项或关联密文中。
        "derived_tag_rows_with_user_tag_ids": scalar(
            &mut conn,
            "SELECT COUNT(*) AS n FROM search_entry_tag WHERE tag_id IN (SELECT tag_id FROM history_tag)"
        ),
        "postings_equal_to_user_tag_ids": scalar(
            &mut conn,
            "SELECT COUNT(*) AS n FROM search_posting p JOIN history_tag t \
             ON p.term_tag = CAST(t.tag_id AS BLOB) OR instr(p.term_tag, CAST(t.tag_id AS BLOB)) > 0"
        ),
        "assignment_ciphertexts_containing_tag_ids": scalar(
            &mut conn,
            "SELECT COUNT(*) AS n FROM history_tag_assignment a JOIN history_tag t \
             ON instr(a.tags_ct, CAST(t.tag_id AS BLOB)) > 0"
        ),
        // 用户标签成员只以 HMAC posting（field_mask = 32）进入索引。
        "hmac_history_tag_postings": scalar(
            &mut conn,
            "SELECT COUNT(*) AS n FROM search_posting WHERE field_mask = 32"
        ),
    })
}

/// 在资料目录（含数据库、WAL 与日志）中查找标签名称的明文。
fn plaintext_name_hits(root: &Path) -> Vec<String> {
    let names = [WORK, PERSONAL, ARCHIVE, CAFE, RENAMED_WORK];
    let mut hits = Vec::new();
    for file in files_under(root) {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        for name in names {
            if bytes
                .windows(name.len())
                .any(|window| window == name.as_bytes())
            {
                hits.push(format!("{}: {name}", file.display()));
            }
        }
    }
    hits
}

struct Evidence {
    steps: Vec<Value>,
}

impl Evidence {
    fn step(&mut self, name: &str, detail: Value) {
        self.steps.push(json!({ "step": name, "detail": detail }));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn history_tags_survive_merge_delete_restart_and_index_rebuild() {
    let evidence_dir = std::env::var_os("UC_HISTORY_TAGS_EVIDENCE_DIR").map(PathBuf::from);
    let temp = tempfile::tempdir().unwrap();
    let root = match &evidence_dir {
        Some(dir) => {
            let root = dir.join("engine-root");
            assert!(!root.exists(), "evidence directory must start empty");
            std::fs::create_dir_all(&root).unwrap();
            root
        }
        None => temp.path().to_path_buf(),
    };
    let storage = MemorySecureStorage::default();
    let mut evidence = Evidence { steps: Vec::new() };

    let (engine, _events) = start(&root, &storage).await;
    engine
        .execute(Operation::CreateSpace(CreateSpaceInput {
            device_name: Some("history tags test".into()),
            passphrase: SecretString::new(PASSPHRASE),
            passphrase_confirmation: SecretString::new(PASSPHRASE),
        }))
        .await
        .expect("space is created");

    // 1. 真实历史：四条文本经 Engine 保存并进入索引。
    let e1 = send_text(&engine, "alpha meeting notes").await;
    let e2 = send_text(&engine, "https://example.com/beta").await;
    let e3 = send_text(&engine, "gamma grocery memo").await;
    let e4 = send_text(&engine, "delta travel plan").await;
    wait_for_indexed(&engine, 4).await;
    let mut hashes = BTreeMap::new();
    for entry in [&e1, &e2, &e3, &e4] {
        hashes.insert(entry.clone(), content_hash(&engine, entry).await);
    }
    evidence.step(
        "entries_indexed",
        json!({ "entries": [e1, e2, e3, e4], "content_sha256": hashes }),
    );

    // 2. 创建：同名（大小写、空白、NFC 形式不同）幂等返回已有标签；非法名称拒绝。
    let (work, created) = create(&engine, WORK).await.unwrap();
    assert!(created);
    assert_eq!(work.name.as_deref(), Some(WORK));
    let (again, created_again) = create(&engine, &format!("  {}  ", WORK.to_lowercase()))
        .await
        .unwrap();
    assert!(!created_again);
    assert_eq!(again.tag_id, work.tag_id);
    let (cafe, _) = create(&engine, CAFE_DECOMPOSED).await.unwrap();
    assert_eq!(cafe.name.as_deref(), Some(CAFE), "trim + NFC");
    let (cafe_again, cafe_created) = create(&engine, &CAFE.to_uppercase()).await.unwrap();
    assert!(!cafe_created);
    assert_eq!(cafe_again.tag_id, cafe.tag_id);
    for invalid in ["   ", "bad\u{7}name", &"界".repeat(65)] {
        assert_eq!(
            code(create(&engine, invalid).await),
            HISTORY_INVALID_INPUT_CODE
        );
    }
    let (personal, _) = create(&engine, PERSONAL).await.unwrap();
    let (archive, _) = create(&engine, ARCHIVE).await.unwrap();
    assert_ne!(work.tag_id, personal.tag_id);
    assert!(
        !work.tag_id.contains(WORK),
        "id is not derived from the name"
    );
    evidence.step(
        "tags_created",
        json!({
            "work": work.tag_id, "personal": personal.tag_id,
            "archive": archive.tag_id, "cafe": cafe.tag_id,
            "duplicate_create_returned_existing": true,
            "invalid_names_rejected_with": HISTORY_INVALID_INPUT_CODE,
        }),
    );

    // 3. 批量关联：缺失条目跳过，重复调用幂等；未知标签整体 NotFound。
    let missing = "00000000-0000-0000-0000-000000000000";
    let batch = change_entries(&engine, true, &work.tag_id, &[&e1, &e2, missing])
        .await
        .unwrap();
    assert_eq!((batch.changed, batch.unchanged), (2, 0));
    assert_eq!(batch.missing_entry_ids, vec![missing.to_string()]);
    let repeat = change_entries(&engine, true, &work.tag_id, &[&e1, &e2])
        .await
        .unwrap();
    assert_eq!((repeat.changed, repeat.unchanged), (0, 2));
    let personal_batch = change_entries(&engine, true, &personal.tag_id, &[&e2, &e3])
        .await
        .unwrap();
    assert_eq!(personal_batch.changed, 2);
    assert_eq!(
        code(change_entries(&engine, true, "no-such-tag", &[&e1]).await),
        HISTORY_NOT_FOUND_CODE
    );
    assert_eq!(
        code(change_entries(&engine, true, &work.tag_id, &[]).await),
        HISTORY_INVALID_INPUT_CODE
    );
    evidence.step(
        "entries_tagged",
        json!({ "first": [batch.changed, batch.unchanged, batch.missing_entry_ids],
                "repeat": [repeat.changed, repeat.unchanged] }),
    );

    // 4. 搜索：过滤（任一/全部）、结果 tags、计数与搜索标签列表覆盖自定义标签。
    let mut work_entries = vec![e1.clone(), e2.clone()];
    work_entries.sort();
    assert_eq!(
        entry_ids(&engine, tagged(&[&work.tag_id], None)).await,
        work_entries
    );
    assert_eq!(
        entry_ids(
            &engine,
            tagged(&[&work.tag_id, &personal.tag_id], Some("all"))
        )
        .await,
        vec![e2.clone()]
    );
    assert_eq!(
        entry_ids(&engine, tagged(&[&personal.tag_id, "link"], Some("all"))).await,
        vec![e2.clone()],
        "user tags combine with builtin tags"
    );
    assert_eq!(
        count(&engine, tagged(&[&work.tag_id, &personal.tag_id], None)).await,
        3
    );
    let e2_tags = search(&engine, tagged(&[&work.tag_id], None))
        .await
        .items
        .into_iter()
        .find(|item| item.entry_id == e2)
        .unwrap()
        .tags;
    for expected in ["link", work.tag_id.as_str(), personal.tag_id.as_str()] {
        assert!(
            e2_tags.iter().any(|tag| tag == expected),
            "{expected} on e2"
        );
    }
    assert_eq!(
        search_tag_count(&engine, &work.tag_id).await,
        Some((2, false))
    );
    evidence.step(
        "search_by_tag",
        json!({ "work_entries": work_entries, "e2_tags": e2_tags }),
    );

    // 5. 多条目汇总：只计仍存在的条目。
    let OperationResult::HistoryEntryTags(summary) = engine
        .execute(Operation::SummarizeHistoryEntryTags(
            HistoryEntryTagsInput {
                entry_ids: vec![e1.clone(), e2.clone(), e3.clone(), missing.into()],
            },
        ))
        .await
        .unwrap()
    else {
        panic!("expected entry tag summary")
    };
    assert_eq!(summary.selected, 3);
    let applied: BTreeMap<_, _> = summary
        .tags
        .iter()
        .map(|tag| (tag.tag_id.clone(), tag.applied))
        .collect();
    assert_eq!(applied.get(&work.tag_id), Some(&2));
    assert_eq!(applied.get(&personal.tag_id), Some(&2));

    // 6. 改名：与其他标签同名返回冲突且不写入；仅改变大小写允许。
    let rename = |tag_id: &str, name: &str| {
        Operation::RenameHistoryTag(RenameHistoryTagInput {
            tag_id: tag_id.into(),
            name: name.into(),
        })
    };
    match engine
        .execute(rename(&personal.tag_id, RENAMED_WORK))
        .await
        .unwrap()
    {
        OperationResult::HistoryTagRenamed(HistoryTagRenameSummary::NameConflict {
            existing_tag_id,
        }) => assert_eq!(existing_tag_id, work.tag_id),
        other => panic!("expected a name conflict: {other:?}"),
    }
    match engine
        .execute(rename(&work.tag_id, RENAMED_WORK))
        .await
        .unwrap()
    {
        OperationResult::HistoryTagRenamed(HistoryTagRenameSummary::Renamed(tag)) => {
            assert_eq!(tag.name.as_deref(), Some(RENAMED_WORK));
            assert_eq!(tag.entry_count, 2);
        }
        other => panic!("expected a rename: {other:?}"),
    }
    assert_eq!(
        code(engine.execute(rename("no-such-tag", "Other")).await),
        HISTORY_NOT_FOUND_CODE
    );
    assert_eq!(
        tag_by_id(&list_tags(&engine).await, &personal.tag_id)
            .unwrap()
            .name
            .as_deref(),
        Some(PERSONAL),
        "a conflicting rename writes nothing"
    );

    // 7. 合并：来源关联并入目标并去重，来源删除；重复合并返回 NotFound；内容不变。
    let merge = |sources: Vec<String>, target: &str| {
        Operation::MergeHistoryTags(MergeHistoryTagsInput {
            source_tag_ids: sources,
            target_tag_id: target.into(),
        })
    };
    assert_eq!(
        code(
            engine
                .execute(merge(vec![work.tag_id.clone()], &work.tag_id))
                .await
        ),
        HISTORY_INVALID_INPUT_CODE
    );
    assert_eq!(
        code(
            engine
                .execute(merge(
                    vec![personal.tag_id.clone(), "no-such-tag".into()],
                    &work.tag_id
                ))
                .await
        ),
        HISTORY_NOT_FOUND_CODE
    );
    assert!(
        tag_by_id(&list_tags(&engine).await, &personal.tag_id).is_some(),
        "a failed merge changes nothing"
    );
    let OperationResult::HistoryTagsMerged(merged) = engine
        .execute(merge(
            vec![personal.tag_id.clone(), archive.tag_id.clone()],
            &work.tag_id,
        ))
        .await
        .unwrap()
    else {
        panic!("expected a merge result")
    };
    assert_eq!((merged.moved, merged.already_on_target), (1, 1));
    assert_eq!(
        code(
            engine
                .execute(merge(vec![personal.tag_id.clone()], &work.tag_id))
                .await
        ),
        HISTORY_NOT_FOUND_CODE,
        "a repeated merge reports the source as gone"
    );
    let after_merge = list_tags(&engine).await;
    assert!(tag_by_id(&after_merge, &personal.tag_id).is_none());
    assert!(tag_by_id(&after_merge, &archive.tag_id).is_none());
    assert_eq!(
        tag_by_id(&after_merge, &work.tag_id).unwrap().entry_count,
        3
    );
    assert!(
        entry_ids(&engine, tagged(&[&personal.tag_id], None))
            .await
            .is_empty(),
        "a merged-away tag filters to nothing without an error"
    );
    assert_eq!(search_tag_count(&engine, &personal.tag_id).await, None);
    assert_eq!(count(&engine, tagged(&[&work.tag_id], None)).await, 3);
    evidence.step(
        "merged",
        json!({ "moved": merged.moved, "already_on_target": merged.already_on_target,
                "work_count": 3, "database": database_summary(&root) }),
    );

    // 8. 移除、条目删除级联、删除标签保留内容。
    let removed = change_entries(&engine, false, &work.tag_id, &[&e1, missing])
        .await
        .unwrap();
    assert_eq!((removed.changed, removed.unchanged), (1, 0));
    engine
        .execute(Operation::DeleteHistoryEntry(HistoryEntryInput {
            entry_id: e3.clone(),
        }))
        .await
        .unwrap();
    assert_eq!(
        tag_by_id(&list_tags(&engine).await, &work.tag_id)
            .unwrap()
            .entry_count,
        1,
        "deleting an entry drops its associations"
    );
    change_entries(&engine, true, &cafe.tag_id, &[&e4])
        .await
        .unwrap();
    let OperationResult::HistoryTagDeleted(deleted) = engine
        .execute(Operation::DeleteHistoryTag(HistoryTagInput {
            tag_id: cafe.tag_id.clone(),
        }))
        .await
        .unwrap()
    else {
        panic!("expected a delete result")
    };
    assert_eq!(deleted.detached, 1);
    assert_eq!(
        code(
            engine
                .execute(Operation::DeleteHistoryTag(HistoryTagInput {
                    tag_id: cafe.tag_id.clone()
                }))
                .await
        ),
        HISTORY_NOT_FOUND_CODE
    );
    wait_for_indexed(&engine, 3).await;
    for entry in [&e1, &e2, &e4] {
        assert_eq!(&content_hash(&engine, entry).await, &hashes[entry]);
    }
    evidence.step(
        "removed_and_deleted",
        json!({ "removed": removed.changed, "detached": deleted.detached,
                "deleted_entry": e3, "database": database_summary(&root) }),
    );

    // 9. 锁定后全部标签操作失败关闭；解锁恢复。
    engine.execute(Operation::LockEncryption).await.unwrap();
    assert_eq!(
        code(engine.execute(Operation::ListHistoryTags).await),
        HISTORY_TAGS_LOCKED_CODE
    );
    assert_eq!(
        code(create(&engine, "Locked").await),
        HISTORY_TAGS_LOCKED_CODE
    );
    assert_eq!(
        code(change_entries(&engine, true, &work.tag_id, &[&e1]).await),
        HISTORY_TAGS_LOCKED_CODE
    );
    unlock(&engine).await;
    assert_eq!(list_tags(&engine).await.len(), 1);
    evidence.step("locked", json!({ "locked_code": HISTORY_TAGS_LOCKED_CODE }));

    // 10. 重启：标签、名称与关联从持久化恢复。
    engine.shutdown(Duration::from_secs(15)).await.unwrap();
    let (engine, _events) = start(&root, &storage).await;
    unlock(&engine).await;
    wait_for_indexed(&engine, 3).await;
    let restarted = list_tags(&engine).await;
    assert_eq!(restarted.len(), 1);
    assert_eq!(restarted[0].tag_id, work.tag_id);
    assert_eq!(restarted[0].name.as_deref(), Some(RENAMED_WORK));
    assert_eq!(restarted[0].entry_count, 1);
    assert_eq!(
        entry_ids(&engine, tagged(&[&work.tag_id], None)).await,
        vec![e2.clone()]
    );
    evidence.step(
        "restarted",
        json!({ "tags": restarted.iter().map(|tag| json!({
            "tag_id": tag.tag_id, "name_readable": tag.name.is_some(), "entry_count": tag.entry_count
        })).collect::<Vec<_>>() }),
    );

    // 11. 显式重建索引后，用户标签过滤、结果 tags 与计数保持一致。
    match engine.execute(Operation::RebuildSearchIndex).await {
        Ok(OperationResult::SearchRebuildAccepted { .. }) => {}
        other => panic!("unexpected rebuild result: {other:?}"),
    }
    wait_for_indexed(&engine, 3).await;
    assert_eq!(
        entry_ids(&engine, tagged(&[&work.tag_id], None)).await,
        vec![e2.clone()]
    );
    assert_eq!(count(&engine, tagged(&[&work.tag_id], None)).await, 1);
    assert_eq!(
        search_tag_count(&engine, &work.tag_id).await,
        Some((1, false))
    );
    for entry in [&e1, &e2, &e4] {
        assert_eq!(&content_hash(&engine, entry).await, &hashes[entry]);
    }
    let database = database_summary(&root);
    assert_eq!(database["history_tag_rows"], 1);
    assert_eq!(database["history_tag_assignment_rows"], 1);
    assert_eq!(database["hmac_history_tag_postings"], 1);
    for leak in [
        "derived_tag_rows_with_user_tag_ids",
        "postings_equal_to_user_tag_ids",
        "assignment_ciphertexts_containing_tag_ids",
    ] {
        assert_eq!(database[leak], 0, "{leak}");
    }
    evidence.step("rebuilt", json!({ "database": database }));

    // 11b. 名称密文无法打开时：列表降级为无名称，改名被拒绝且不改写密文，删除仍可用。
    let mut conn = SqliteConnection::establish(&profile_database(&root).to_string_lossy())
        .expect("open profile database");
    diesel::sql_query(format!(
        "UPDATE history_tag SET payload_ct = X'00' WHERE tag_id = '{}'",
        work.tag_id
    ))
    .execute(&mut conn)
    .expect("corrupt tag payload");
    let unreadable = list_tags(&engine).await;
    let unreadable = tag_by_id(&unreadable, &work.tag_id).expect("unreadable tag listed");
    assert_eq!(unreadable.name, None);
    assert_eq!(unreadable.entry_count, 1);
    assert_eq!(
        code(engine.execute(rename(&work.tag_id, "Recovered")).await),
        HISTORY_FAILED_CODE
    );
    assert_eq!(
        scalar(
            &mut conn,
            &format!(
                "SELECT COUNT(*) AS n FROM history_tag WHERE tag_id = '{}' AND payload_ct = X'00'",
                work.tag_id
            ),
        ),
        1,
        "rejected rename must not reseal the unreadable payload"
    );
    drop(conn);
    engine
        .execute(Operation::DeleteHistoryTag(HistoryTagInput {
            tag_id: work.tag_id.clone(),
        }))
        .await
        .expect("unreadable tag can be deleted");
    assert!(tag_by_id(&list_tags(&engine).await, &work.tag_id).is_none());
    evidence.step(
        "unreadable_payload",
        json!({ "rename_code": HISTORY_FAILED_CODE, "deleted": true }),
    );

    engine.shutdown(Duration::from_secs(15)).await.unwrap();

    // 12. 资料目录（数据库、WAL、日志）中不存在任何标签名称明文。
    let hits = plaintext_name_hits(&root);
    assert!(hits.is_empty(), "tag names found in plaintext: {hits:?}");
    evidence.step(
        "plaintext_scan",
        json!({ "files_scanned": files_under(&root).len(), "hits": hits }),
    );

    if let Some(dir) = evidence_dir {
        let report = json!({
            "test": "history_tags_survive_merge_delete_restart_and_index_rebuild",
            "steps": evidence.steps,
        });
        std::fs::write(
            dir.join("evidence.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
    }
}
