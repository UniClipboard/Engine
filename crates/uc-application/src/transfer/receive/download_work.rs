//! 单项的网络及本地结算共同持有同一份许可；不会再次入队或申请额度。
use super::scheduler::{DownloadPermit, ScheduleError, ScheduleRefusal};
use crate::transfer::blob::transport::BlobRetainedContent;
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Clone, Default)]
pub(in super::super) struct DownloadSettlement(Arc<Mutex<OwnedDownload>>);

#[derive(Default)]
struct OwnedDownload {
    // 内容保护及 Uni 本地工作先释放，最后才归还调度额度。
    local: Option<Box<dyn BlobRetainedContent>>,
    permit: Option<DownloadPermit>,
}

impl DownloadSettlement {
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
