use std::time::Duration;

use crate::ports::{BudgetStatus, ElapsedBudget};

/// 终态从产生起的最长保留时间。
pub const TERMINAL_RETENTION: Duration = Duration::from_secs(600);
/// 单轮拨号、等待额度、发送与等待 ACK 的合计上限。
pub const TERMINAL_ROUND_LIMIT: Duration = Duration::from_secs(5);
/// 网络恢复提前发送时，距前次实际启动的最小间隔。
const EARLY_GAP: Duration = Duration::from_secs(2);
/// 相对终态产生时刻的默认发送时点，含首次共 8 次。
const SLOT_OFFSETS_SECS: [u64; 8] = [0, 2, 10, 30, 60, 120, 300, 540];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetransmitStop {
    Acknowledged,
    Expired,
    Exhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetransmitPoll {
    /// 启动新一轮；`deadline` 是本轮的绝对截止（经过时间读数）。
    Start {
        deadline: Duration,
    },
    /// 前一轮仍在执行，不得重叠。
    Busy,
    /// 暂无资格，最早在此读数重新检查。
    WaitUntil(Duration),
    Stopped(RetransmitStop),
}

/// 单个终态的串行补发状态。
///
/// 同项最多一轮在途；每次启动消费此刻之前所有已到期时点，挂起恢复不追补历史轮次。
/// 网络恢复最多提前消费一个未到期时点，重复机会合并。
#[derive(Debug, Clone)]
pub struct TerminalRetransmit {
    generated: Option<Duration>,
    budget: ElapsedBudget,
    next_slot: usize,
    last_start: Option<Duration>,
    in_flight: bool,
    early_pending: bool,
    stopped: Option<RetransmitStop>,
}

impl TerminalRetransmit {
    /// `generated` 为终态产生时的经过时间读数；读不到则立即视为过期。
    pub fn new(generated: Option<Duration>) -> Self {
        Self {
            generated,
            budget: ElapsedBudget::start(generated, TERMINAL_RETENTION),
            next_slot: 0,
            last_start: None,
            in_flight: false,
            early_pending: false,
            stopped: None,
        }
    }

    pub fn poll(&mut self, now: Option<Duration>) -> RetransmitPoll {
        if let Some(stop) = self.stopped {
            return RetransmitPoll::Stopped(stop);
        }
        let (Some(generated), Some(now)) = (self.generated, now) else {
            return self.stop(RetransmitStop::Expired);
        };
        let regressed = self.last_start.is_some_and(|start| now < start);
        if regressed || self.budget.status(Some(now)) == BudgetStatus::Expired {
            return self.stop(RetransmitStop::Expired);
        }
        if self.in_flight {
            return RetransmitPoll::Busy;
        }
        let Some(offset) = SLOT_OFFSETS_SECS.get(self.next_slot) else {
            return self.stop(RetransmitStop::Exhausted);
        };
        let slot_at = generated + Duration::from_secs(*offset);
        let early_at = self
            .early_pending
            .then(|| self.last_start.map(|start| start + EARLY_GAP))
            .flatten();
        let due = slot_at <= now;
        let early = early_at.is_some_and(|at| at <= now);
        if !(due || early) {
            let wake = early_at.map_or(slot_at, |at| at.min(slot_at));
            return RetransmitPoll::WaitUntil(wake.min(generated + TERMINAL_RETENTION));
        }

        let before = self.next_slot;
        while SLOT_OFFSETS_SECS
            .get(self.next_slot)
            .is_some_and(|offset| generated + Duration::from_secs(*offset) <= now)
        {
            self.next_slot += 1;
        }
        if self.next_slot == before {
            self.next_slot += 1;
        }
        self.early_pending = false;
        self.last_start = Some(now);
        self.in_flight = true;
        RetransmitPoll::Start {
            deadline: (now + TERMINAL_ROUND_LIMIT).min(generated + TERMINAL_RETENTION),
        }
    }

    /// 本轮结束（成功发送未获 ACK、超时或失败均同样结束，不关闭其他任务使用的连接）。
    pub fn finish_round(&mut self) {
        self.in_flight = false;
    }

    /// 收到精确匹配的 ACK 后永久停止。
    pub fn acknowledge(&mut self) {
        self.stopped.get_or_insert(RetransmitStop::Acknowledged);
    }

    /// 网络恢复机会；多次恢复合并为至多一个提前时点。
    pub fn network_recovered(&mut self) {
        self.early_pending = true;
    }

    fn stop(&mut self, reason: RetransmitStop) -> RetransmitPoll {
        RetransmitPoll::Stopped(*self.stopped.get_or_insert(reason))
    }
}
