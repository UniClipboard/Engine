//! 可用性读取失败不能提升为已持有，也不能继续下载或覆盖原条目。
use super::{build, fixture_input, MockCapture, MockEntryRepo, MockWrite};
use crate::clipboard::sync::apply_inbound::ApplyInboundError;
use async_trait::async_trait;
use std::io::{Error, ErrorKind};
use std::sync::Arc;
use uc_core::clipboard::ClipboardRepositoryError;
use uc_core::ids::EntryId;
use uc_core::ports::clipboard::CheckEntryAvailabilityPort;

struct UnreadableAvailability;
#[async_trait]
impl CheckEntryAvailabilityPort for UnreadableAvailability {
    async fn is_entry_available(&self, _: &EntryId) -> Result<bool, ClipboardRepositoryError> {
        Err(ClipboardRepositoryError::Storage(Box::new(Error::from(
            ErrorKind::PermissionDenied,
        ))))
    }
}

#[tokio::test]
async fn availability_failure_preserves_source_and_never_resurfaces_or_replaces_entry() {
    let mut repo = MockEntryRepo::new();
    repo.expect_find_entry_id_by_snapshot_hash()
        .times(1)
        .returning(|_| Ok(Some(EntryId::from("held"))));
    let mut capture = MockCapture::new();
    capture.expect_capture().times(0);
    let mut write = MockWrite::new();
    write.expect_write().times(0);
    let apply =
        build(repo, capture, write).with_check_entry_availability(Arc::new(UnreadableAvailability));
    let (input, _) = fixture_input("availability proof");
    let error = apply.execute(input).await.unwrap_err();
    let ApplyInboundError::DedupQuery(source) = error else {
        panic!("availability source required")
    };
    assert_eq!(
        source
            .chain()
            .find_map(|cause| cause.downcast_ref::<Error>())
            .unwrap()
            .kind(),
        ErrorKind::PermissionDenied
    );
}
