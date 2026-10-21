//! 进程网络日志与 Engine 生命周期的交接。
//!
//! 网络日志属于进程，同一进程可能先后或同时存在多个 Engine。每个工作中（未暂停、未关闭）的 Engine 持有一份登记；
//! 最后一份登记结束时网络日志停车，交还运行目录的分段文件与租约，第一份登记开始时重新打开。宿主不直接停车：
//! 只有 Engine 生命周期负责人在发布暂停或关闭结果之前结束登记，在恢复之前开始登记。

use std::sync::Mutex;
use std::time::Duration;

use crate::local_file::lock;
use crate::runtime::installed_network_log;
use crate::SignalResult;

/// 工作中的 Engine 数；同时串行停车与重开，等待期间不让另一次转换交错。
static ACTIVE_ENGINES: Mutex<usize> = Mutex::new(0);

/// 一个工作中 Engine 的登记。未调用 [`Self::end`] 就被丢弃时只撤销计数，不等待停车。
pub struct EngineLogActivity {
    ended: bool,
}

impl EngineLogActivity {
    /// 登记一个开始工作的 Engine；从零变为一时重开停车中的网络日志。没有网络日志时返回 `None`。
    /// 会等待文件线程，不能在异步运行线程上调用。
    pub fn begin(deadline: Duration) -> (Self, Option<SignalResult>) {
        let mut active = lock(&ACTIVE_ENGINES);
        *active = active.saturating_add(1);
        let reopened = (*active == 1)
            .then(installed_network_log)
            .flatten()
            .map(|network| network.unpark(deadline));
        (Self { ended: false }, reopened)
    }

    /// 结束登记；最后一份登记结束时停车，并等待已接受记录落盘、文件与租约交还。没有网络日志或仍有其他
    /// 工作中的 Engine 时返回 `None`。会等待文件线程，不能在异步运行线程上调用。
    pub fn end(mut self, deadline: Duration) -> Option<SignalResult> {
        self.ended = true;
        let mut active = lock(&ACTIVE_ENGINES);
        *active = active.saturating_sub(1);
        (*active == 0)
            .then(installed_network_log)
            .flatten()
            .map(|network| network.park(deadline))
    }
}

impl Drop for EngineLogActivity {
    fn drop(&mut self) {
        if !self.ended {
            let mut active = lock(&ACTIVE_ENGINES);
            *active = active.saturating_sub(1);
        }
    }
}
