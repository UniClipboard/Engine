//! 单项的网络及本地结算共同持有同一份许可；不会再次入队或申请额度。
use super::scheduler::{DownloadPermit, ScheduleError, ScheduleRefusal};
use super::{LocalSettlement, LocalSettlementOutcome};
use crate::transfer::blob::transport::BlobRetainedContent;
use anyhow::Error as SourceError;
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Clone, Default)]
pub(in super::super) struct DownloadSettlement(Arc<Mutex<OwnedDownload>>);

#[derive(Default)]
struct OwnedDownload {
    // 内容保护及 Uni 本地工作先释放，最后才归还调度额度。
    local: Option<Box<dyn BlobRetainedContent>>,
    permit: Option<DownloadPermit>,
    completion: Option<LocalSettlement>,
}

impl DownloadSettlement {
    pub(in super::super) fn require_local_commit(&self) -> LocalSettlement {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .completion
            .get_or_insert_with(LocalSettlement::default)
            .clone()
    }
    pub(in super::super) fn local_completion(&self) -> Option<LocalSettlement> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .completion
            .clone()
    }
    pub(in super::super) fn complete_local(
        &self,
        result: Result<LocalSettlementOutcome, Arc<SourceError>>,
    ) {
        if let Some(completion) = self.local_completion() {
            completion.finish(result);
        }
    }
    pub(in super::super) fn abandon_local(&self, source: Arc<SourceError>) {
        if let Some(completion) = self.local_completion() {
            completion.abandon(Some(source));
        }
    }

    pub(in super::super) fn retain_permit(
        &self,
        permit: DownloadPermit,
    ) -> Result<(), ScheduleError> {
        let mut retained = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if retained.permit.is_some() {
            return Err(ScheduleError(ScheduleRefusal::Busy));
        }
        retained.permit = Some(permit);
        Ok(())
    }

    pub(in super::super) fn retain_content(
        &self,
        resource: Box<dyn BlobRetainedContent>,
    ) -> Result<(), ScheduleError> {
        let mut retained = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if retained.local.is_some() {
            return Err(ScheduleError(ScheduleRefusal::Busy));
        }
        retained.local = Some(resource);
        Ok(())
    }
}

impl Drop for OwnedDownload {
    fn drop(&mut self) {
        if let Some(completion) = &self.completion {
            completion.abandon(None);
        }
    }
}
