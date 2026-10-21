//! 有界公平下载调度；许可归完整任务持有，网络 future 退出不归还额度。
mod queue;

pub(in super::super) use queue::{
    DownloadPermit, DownloadReservation, DownloadScheduler, ScheduleError, ScheduleRefusal,
};

#[cfg(test)]
mod tests;
