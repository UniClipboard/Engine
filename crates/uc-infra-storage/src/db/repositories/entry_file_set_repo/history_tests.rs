//! 真实SQLite限定发送归属、内容类别与读取额度；读取不写任何新历史。
use super::tests::{make_repo, sample_file_set, seed_blob, seed_entry};
use crate::db::repositories::DieselEntryDeliveryRepository;
use crate::db::{
    ports::DbExecutor,
    schema::{clipboard_entry, clipboard_event},
};
use diesel::{ExpressionMethods, QueryDsl, RunQueryDsl};
use uc_application::deps::ReadProgressHistoryPort;
use uc_core::clipboard::{EntryDeliveryRecord, EntryDeliveryStatus};
use uc_core::ids::{DeviceId, EntryId};
use uc_core::ports::{clipboard::EntryFileSetRepositoryPort, EntryDeliveryRepositoryPort};

#[tokio::test]
async fn history_requires_local_unremoved_file_ownership_and_preserves_authenticated_manifest() {
    let (repo, executor, _root) = make_repo();
    let deliveries = DieselEntryDeliveryRepository::new(executor.clone());
    let local = DeviceId::new("test-device");
    let peer = DeviceId::new("peer");
    let entry = EntryId::from("entry");
    seed_entry(&executor, "entry");
    seed_blob(&executor, "blob-a");
    let manifest = sample_file_set();
    repo.save(&entry, &manifest).await.unwrap();
    assert!(repo
        .read_owned(local, peer, &entry)
        .await
        .unwrap()
        .is_none());
    deliveries
        .record_attempt(&EntryDeliveryRecord {
            entry_id: entry.clone(),
            target_device_id: peer,
            status: EntryDeliveryStatus::Delivered,
            reason_detail: None,
            updated_at_ms: 1,
        })
        .await
        .unwrap();
    assert_eq!(
        repo.read_owned(local, peer, &entry)
            .await
            .unwrap()
            .unwrap()
            .file_set
            .unwrap(),
        manifest
    );
    assert!(repo
        .read_owned(DeviceId::new("other"), peer, &entry)
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .read_owned(local, DeviceId::new("other"), &entry)
        .await
        .unwrap()
        .is_none());
    executor
        .run(|conn| {
            diesel::update(clipboard_event::table)
                .set(clipboard_event::source_device.eq("remote"))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    assert!(repo
        .read_owned(local, peer, &entry)
        .await
        .unwrap()
        .is_none());
    executor
        .run(|conn| {
            diesel::update(clipboard_event::table)
                .set(clipboard_event::source_device.eq("test-device"))
                .execute(conn)?;
            diesel::update(clipboard_entry::table)
                .set(clipboard_entry::deleted_at_ms.eq(Some(2_i64)))
                .execute(conn)?;
            Ok(())
        })
        .unwrap();
    assert!(repo
        .read_owned(local, peer, &entry)
        .await
        .unwrap()
        .is_none());
    assert!(repo.list_owned(local, &[peer]).await.unwrap().is_empty());
}

#[tokio::test]
async fn history_caps_file_scopes_per_peer_and_globally_without_text_crowding() {
    let (repo, executor, _root) = make_repo();
    let deliveries = DieselEntryDeliveryRepository::new(executor.clone());
    let local = DeviceId::new("test-device");
    let peers = (0..5)
        .map(|index| DeviceId::new(format!("peer-{index}")))
        .collect::<Vec<_>>();
    for index in 0..67 {
        let entry = format!("entry-{index:03}");
        seed_entry(&executor, &entry);
        for peer in &peers {
            deliveries
                .record_attempt(&EntryDeliveryRecord {
                    entry_id: EntryId::from(entry.clone()),
                    target_device_id: *peer,
                    status: EntryDeliveryStatus::Delivered,
                    reason_detail: None,
                    updated_at_ms: index,
                })
                .await
                .unwrap();
        }
    }
    let listed = repo.list_owned(local, &peers).await.unwrap();
    assert_eq!(listed.len(), 256);
    for peer in &peers {
        assert!(listed.iter().filter(|(target, _)| target == peer).count() <= 64);
    }
    let first = repo.list_owned(local, &[peers[0]]).await.unwrap();
    assert_eq!(first.len(), 64);
    assert_eq!(first[0].1, EntryId::from("entry-066"));
    executor
        .run(|conn| {
            diesel::update(
                clipboard_entry::table.filter(clipboard_entry::entry_id.eq("entry-066")),
            )
            .set(clipboard_entry::content_category.eq("text"))
            .execute(conn)?;
            Ok(())
        })
        .unwrap();
    let first = repo.list_owned(local, &[peers[0]]).await.unwrap();
    assert_eq!(first.len(), 64);
    assert_eq!(first[0].1, EntryId::from("entry-065"));
}
