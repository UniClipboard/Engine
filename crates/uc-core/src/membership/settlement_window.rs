/// 本机收尾期限。
///
/// 终态之后仍需完成的通知、撤销与清理都在有限时间内收尾：到期后本机结束该项责任，
/// 不再依赖对端确认，也不无限重试。与配对尝试契约不同，本类型只表达本机的收尾边界，
/// 不参与双方协商，也不要求固定时长。
///
/// 起点可以延后记录：持久记录在首次处理该项收尾时才保存起点，此前处于未起算状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettlementWindow {
    started_at_ms: Option<i64>,
    duration_ms: i64,
}

/// 收尾期限在某一时刻的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettlementWindowState {
    /// 尚未记录起点，本轮记录起点后照常收尾。
    Unstarted,
    /// 仍在期限内。
    Open { deadline_ms: i64 },
    /// 已到期，本机结束该项责任。
    Expired,
}

impl SettlementWindow {
    /// 从持久记录恢复；非正起点表示尚未起算。
    pub const fn from_stored_start(duration_ms: i64, started_at_ms: i64) -> Self {
        Self {
            started_at_ms: if started_at_ms > 0 {
                Some(started_at_ms)
            } else {
                None
            },
            duration_ms,
        }
    }

    /// 沿用既有截止时间，不另起窗口；用于收尾责任与既定期限共用边界的情形。
    pub const fn until(deadline_ms: i64) -> Self {
        Self {
            started_at_ms: Some(deadline_ms),
            duration_ms: 0,
        }
    }

    /// 记录起点；已起算的期限不重新计时。
    pub const fn started(self, now_ms: i64) -> Self {
        match self.started_at_ms {
            Some(_) => self,
            None => Self {
                started_at_ms: Some(now_ms),
                duration_ms: self.duration_ms,
            },
        }
    }

    pub const fn started_at_ms(self) -> Option<i64> {
        self.started_at_ms
    }

    /// 起点溢出时按已到期处理，不补造更晚的期限。
    pub const fn deadline_ms(self) -> Option<i64> {
        match self.started_at_ms {
            Some(started_at_ms) => started_at_ms.checked_add(self.duration_ms),
            None => None,
        }
    }

    pub const fn state(self, now_ms: i64) -> SettlementWindowState {
        match self.started_at_ms {
            None => SettlementWindowState::Unstarted,
            Some(_) => match self.deadline_ms() {
                Some(deadline_ms) if now_ms < deadline_ms => {
                    SettlementWindowState::Open { deadline_ms }
                }
                _ => SettlementWindowState::Expired,
            },
        }
    }

    pub const fn is_expired(self, now_ms: i64) -> bool {
        matches!(self.state(now_ms), SettlementWindowState::Expired)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_window_starts_on_first_handling_and_ends_at_its_deadline() {
        let window = SettlementWindow::from_stored_start(300_000, 0);
        assert_eq!(window.state(1_000), SettlementWindowState::Unstarted);
        assert_eq!(window.deadline_ms(), None);

        let started = window.started(1_000);
        assert_eq!(
            started.state(1_000),
            SettlementWindowState::Open {
                deadline_ms: 301_000
            }
        );
        assert_eq!(started.started(9_999), started);
        assert_eq!(started.state(300_999).is_open(), true);
        assert!(started.is_expired(301_000));
    }

    #[test]
    fn an_inherited_deadline_does_not_open_a_second_window() {
        let window = SettlementWindow::until(500);
        assert_eq!(window.deadline_ms(), Some(500));
        assert_eq!(
            window.state(499),
            SettlementWindowState::Open { deadline_ms: 500 }
        );
        assert!(window.is_expired(500));
    }

    #[test]
    fn an_overflowing_start_counts_as_expired() {
        let window = SettlementWindow::from_stored_start(300_000, i64::MAX);
        assert!(window.is_expired(0));
    }

    impl SettlementWindowState {
        const fn is_open(self) -> bool {
            matches!(self, Self::Open { .. })
        }
    }
}
