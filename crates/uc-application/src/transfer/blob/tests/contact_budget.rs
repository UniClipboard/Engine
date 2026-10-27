//! 用原进度接收负责人持续确认联系；下载仍使用原FetchBlobUseCase与实际期限。
use crate::connectivity::{ElapsedClockError, ElapsedClockPort};
use crate::transfer::progress::{
    CurrentProgressTaskPort, ProgressBound, ProgressContactNotice, ProgressReceiveError,
    ProgressReceiverPort, ProgressTaskDescriptor, TransferProgressReceiver,
};
use crate::transfer::receive::timing::{ReceiveTiming, TimingError, TimingRefusal};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{advance, Instant};
use uc_core::file_transfer::{
    ProgressFreshnessView, ProgressIdentity, ProgressItem, ProgressItemKind,
};
use uc_core::ids::DeviceId;

struct Clock(Instant);
impl ElapsedClockPort for Clock {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        Ok(Instant::now().saturating_duration_since(self.0))
    }
}
struct Current(ProgressTaskDescriptor);
#[async_trait]
impl CurrentProgressTaskPort for Current {
    async fn challenge(
        &mut self,
        _: ProgressIdentity,
    ) -> Result<ProgressTaskDescriptor, ProgressReceiveError> {
        Ok(self.0)
    }
}
pub(super) struct ContactProbe {
    receiver: TransferProgressReceiver,
    peer: DeviceId,
    bound: ProgressBound,
}
impl ContactProbe {
    pub(super) async fn new(peer: DeviceId, source: ProgressIdentity) -> Self {
        let receiver = TransferProgressReceiver::new(
            peer,
            source,
            vec![ProgressItem {
                kind: ProgressItemKind::File,
                index: 0,
            }],
            Arc::new(Clock(Instant::now())),
        )
        .unwrap();
        let bound = receiver
            .register(
                &peer,
                1,
                source,
                &mut Current(ProgressTaskDescriptor {
                    producer: ProgressIdentity::new([2; 16]),
                    revision: 1,
                    source,
                    attempt: ProgressIdentity::new([3; 16]),
                    item_count: 1,
                }),
            )
            .await
            .unwrap();
        Self {
            receiver,
            peer,
            bound,
        }
    }
    pub(super) async fn pulse(&self, sequence: u64) {
        self.receiver
            .adopt_contact(
                &self.peer,
                1,
                ProgressContactNotice {
                    binding: self.bound.binding,
                    source: self.bound.descriptor.source,
                    attempt: self.bound.descriptor.attempt,
                    revision: self.bound.descriptor.revision,
                    sequence,
                },
            )
            .await
            .unwrap();
        let snapshot = self.receiver.snapshot();
        assert_eq!(snapshot.freshness.unwrap(), ProgressFreshnessView::Live);
        assert!(snapshot.items.iter().all(|(_, state)| state.is_none()));
    }
}

#[tokio::test(start_paused = true)]
async fn valid_contacts_and_duplicate_byte_reports_cannot_extend_the_original_data_stall_budget() {
    let contacts = ContactProbe::new(DeviceId::new("peer"), ProgressIdentity::new([4; 16])).await;
    let started = Instant::now();
    let mut timing = ReceiveTiming::new(started);
    assert!(timing.validated_through(100, started).unwrap());
    let deadline = timing.deadline().at;
    let mut sequence = 0;
    while Instant::now() < deadline {
        advance(Duration::from_secs(10).min(deadline - Instant::now())).await;
        sequence += 1;
        contacts.pulse(sequence).await;
        assert_eq!(timing.deadline().at, deadline);
        if Instant::now() < deadline {
            assert!(!timing.validated_through(100, Instant::now()).unwrap());
        }
    }
    assert!(matches!(
        timing.check(Instant::now()),
        Err(TimingError(TimingRefusal::DataStalled))
    ));
}
