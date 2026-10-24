//! 单项活动值与终态的纯规则；真实新round可回落字节，终态始终封口。
use super::{
    ProgressActive, ProgressItemState, ProgressTerminal, ProgressTerminalEffect,
    ProgressTerminalSettlement,
};

#[derive(Default)]
pub struct ProgressItemProjection {
    active: Option<ProgressActive>,
    terminal: ProgressTerminalSettlement,
}
pub enum ProgressItemInput {
    Update(ProgressActive),
    Terminal(ProgressTerminal),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressItemEffect {
    /// 保存新投影后才可发布变化；终态还可回复精确确认。
    Adopted,
    /// 不发布变化，精确重复终态仍可确认。
    Duplicate,
    /// 不修改事实、不发布变化、不确认被拒绝的输入。
    Stale,
    /// 保留既有终态，不发布变化或确认冲突结果。
    Conflict,
    /// 非法输入不改变投影，不发布变化或确认。
    Invalid,
}
impl ProgressItemProjection {
    pub fn state(&self) -> Option<ProgressItemState> {
        self.terminal
            .terminal()
            .map(ProgressItemState::Terminal)
            .or_else(|| self.active.map(ProgressItemState::Active))
    }
    pub fn apply(&mut self, input: ProgressItemInput) -> ProgressItemEffect {
        let item = match &input {
            ProgressItemInput::Update(value) => value.item,
            ProgressItemInput::Terminal(value) => value.item,
        };
        if self.state().is_some_and(|current| current.item() != item) {
            return ProgressItemEffect::Invalid;
        }
        match input {
            ProgressItemInput::Terminal(result) => {
                // 终态由独立result_revision确认，不能用活动值的序号压过已结算事实。
                if self.terminal.terminal().is_none()
                    && self
                        .active
                        .is_some_and(|active| result.round < active.round)
                {
                    return ProgressItemEffect::Stale;
                }
                match self.terminal.apply(result) {
                    ProgressTerminalEffect::Adopted => ProgressItemEffect::Adopted,
                    ProgressTerminalEffect::Duplicate => ProgressItemEffect::Duplicate,
                    ProgressTerminalEffect::Conflict => ProgressItemEffect::Conflict,
                }
            }
            ProgressItemInput::Update(update) => {
                if self.terminal.terminal().is_some() {
                    return ProgressItemEffect::Stale;
                }
                if update.total.is_some_and(|total| update.bytes > total) {
                    return ProgressItemEffect::Invalid;
                }
                if let Some(current) = self.active {
                    if update == current {
                        return ProgressItemEffect::Duplicate;
                    }
                    if update.round < current.round
                        || (update.round == current.round
                            && (update.sequence <= current.sequence
                                || update.bytes < current.bytes))
                    {
                        return ProgressItemEffect::Stale;
                    }
                    if update.round == current.round
                        && current.total.is_some()
                        && update.total != current.total
                    {
                        return ProgressItemEffect::Invalid;
                    }
                }
                self.active = Some(update);
                ProgressItemEffect::Adopted
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_transfer::{
        ProgressItem, ProgressItemKind, ProgressPhase, ProgressTerminalOutcome,
    };
    fn update() -> ProgressActive {
        ProgressActive {
            item: ProgressItem {
                kind: ProgressItemKind::File,
                index: 0,
            },
            round: 0,
            sequence: 1,
            bytes: 5,
            total: Some(10),
            phase: ProgressPhase::Transferring,
        }
    }
    #[test]
    fn new_round_can_reset_bytes_but_old_or_duplicate_updates_do_not_advance() {
        let a = update();
        let mut state = ProgressItemProjection::default();
        assert_eq!(
            state.apply(ProgressItemInput::Update(a)),
            ProgressItemEffect::Adopted
        );
        assert_eq!(
            state.apply(ProgressItemInput::Update(a)),
            ProgressItemEffect::Duplicate
        );
        for bad in [
            ProgressActive { sequence: 0, ..a },
            ProgressActive {
                sequence: 2,
                bytes: 4,
                ..a
            },
        ] {
            assert_eq!(
                state.apply(ProgressItemInput::Update(bad)),
                ProgressItemEffect::Stale
            );
        }
        let retry = ProgressActive {
            round: 1,
            sequence: 0,
            bytes: 0,
            phase: ProgressPhase::Retrying,
            ..a
        };
        assert_eq!(
            state.apply(ProgressItemInput::Update(retry)),
            ProgressItemEffect::Adopted
        );
        assert_eq!(
            state.apply(ProgressItemInput::Update(a)),
            ProgressItemEffect::Stale
        );
    }
    #[test]
    fn known_total_and_byte_bounds_are_checked_and_terminal_cannot_be_reopened() {
        let a = update();
        let mut state = ProgressItemProjection::default();
        state.apply(ProgressItemInput::Update(a));
        for total in [None, Some(0), Some(11)] {
            assert_eq!(
                state.apply(ProgressItemInput::Update(ProgressActive {
                    sequence: 2,
                    total,
                    ..a
                })),
                ProgressItemEffect::Invalid
            );
        }
        let result = ProgressTerminal {
            item: a.item,
            round: 0,
            sequence: 2,
            result_revision: 1,
            bytes: 10,
            total: Some(10),
            outcome: ProgressTerminalOutcome::Completed,
        };
        assert_eq!(
            state.apply(ProgressItemInput::Terminal(result)),
            ProgressItemEffect::Adopted
        );
        assert_eq!(
            state.apply(ProgressItemInput::Terminal(result)),
            ProgressItemEffect::Duplicate
        );
        assert_eq!(
            state.apply(ProgressItemInput::Update(ProgressActive {
                round: 2,
                sequence: 50,
                ..a
            })),
            ProgressItemEffect::Stale
        );
        assert_eq!(state.state(), Some(ProgressItemState::Terminal(result)));
    }
    #[test]
    fn an_old_round_terminal_is_stale_but_current_round_result_revision_can_seal_activity() {
        let a = ProgressActive {
            round: 2,
            sequence: 3,
            ..update()
        };
        let mut state = ProgressItemProjection::default();
        state.apply(ProgressItemInput::Update(a));
        let old = ProgressTerminal {
            item: a.item,
            round: 1,
            sequence: 99,
            result_revision: 1,
            bytes: 10,
            total: Some(10),
            outcome: ProgressTerminalOutcome::Completed,
        };
        assert_eq!(
            state.apply(ProgressItemInput::Terminal(old)),
            ProgressItemEffect::Stale
        );
        assert_eq!(
            state.apply(ProgressItemInput::Terminal(ProgressTerminal {
                round: 2,
                sequence: 2,
                ..old
            })),
            ProgressItemEffect::Adopted
        );
        assert!(matches!(
            state.state(),
            Some(ProgressItemState::Terminal(_))
        ));
    }
}
