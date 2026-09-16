use super::HandleMembershipHistoryMessageError;
use crate::space::membership::InboundMembershipTransfer;
use uc_core::ids::DeviceId;
use uc_core::membership::{MembershipHistorySuffixPageV4, MAX_MEMBERSHIP_HISTORY_FRAME_SIZE};

const MAX_TRANSFER_BYTES: usize = MAX_MEMBERSHIP_HISTORY_FRAME_SIZE * 4;

pub(super) enum PageAdmission {
    Rejected,
    Continue {
        next: u32,
        changed: Option<InboundMembershipTransfer>,
    },
    Complete(InboundMembershipTransfer),
}

impl InboundMembershipTransfer {
    /// 一个来源只有一个暂存请求；新请求首帧可取代旧暂存，迟到后续页不能破坏它。
    pub(super) fn accept_page(
        previous: Option<Self>,
        source: DeviceId,
        page: MembershipHistorySuffixPageV4,
    ) -> Result<PageAdmission, HandleMembershipHistoryMessageError> {
        let id = page.transfer_id();
        let count = page.page_count();
        let index = page.page_index();
        let mut transfer = match previous {
            Some(old) if old.source_device_id != source => {
                return Err(HandleMembershipHistoryMessageError::RecoveryRequired)
            }
            Some(old) if old.transfer_id == id && old.page_count != count => {
                return Ok(PageAdmission::Rejected)
            }
            Some(old) if old.transfer_id == id => old,
            Some(_) if index != 0 => {
                return Ok(PageAdmission::Continue {
                    next: 0,
                    changed: None,
                })
            }
            _ => Self {
                source_device_id: source,
                transfer_id: id,
                page_count: count,
                pages: Default::default(),
                total_bytes: 0,
            },
        };
        let next = u32::try_from(transfer.pages.len())
            .map_err(|_| HandleMembershipHistoryMessageError::RecoveryRequired)?;
        match index.cmp(&next) {
            std::cmp::Ordering::Less => {
                return Ok(if transfer.pages.get(&index) == Some(&page) {
                    PageAdmission::Continue {
                        next,
                        changed: None,
                    }
                } else {
                    PageAdmission::Rejected
                })
            }
            std::cmp::Ordering::Greater => {
                return Ok(PageAdmission::Continue {
                    next,
                    changed: None,
                })
            }
            std::cmp::Ordering::Equal => {}
        }
        let bytes = postcard::to_stdvec(&page)
            .map_err(|_| HandleMembershipHistoryMessageError::Rejected)?
            .len();
        let Some(total) = transfer
            .total_bytes
            .checked_add(bytes)
            .filter(|total| *total <= MAX_TRANSFER_BYTES)
        else {
            return Ok(PageAdmission::Rejected);
        };
        transfer.total_bytes = total;
        transfer.pages.insert(index, page);
        if transfer.pages.len() == count as usize {
            Ok(PageAdmission::Complete(transfer))
        } else {
            Ok(PageAdmission::Continue {
                next: next + 1,
                changed: Some(transfer),
            })
        }
    }
}
