//! 进度身份与不可变终态；只表达规则，不含线上编码或本地输出步骤。
use super::FileTransferCancellationReason;
use std::collections::HashSet;
use std::fmt;

pub const MAX_PROGRESS_ITEMS: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgressIdentity([u8; 16]);
impl ProgressIdentity {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    pub const fn bytes(self) -> [u8; 16] {
        self.0
    }
}
impl fmt::Debug for ProgressIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ProgressIdentity(<opaque>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProgressItemKind {
    File,
    Representation,
    DirectoryUnit,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgressItem {
    pub kind: ProgressItemKind,
    pub index: u16,
}
/// 原始逻辑计划的跨项规则；目录必须是唯一完整单元，同名内容不参与身份判定。
pub fn valid_progress_plan(plan: &[ProgressItem]) -> bool {
    let mut unique = HashSet::new();
    !plan.is_empty()
        && plan.len() <= MAX_PROGRESS_ITEMS
        && !plan.iter().any(|item| {
            usize::from(item.index) >= MAX_PROGRESS_ITEMS
                || (item.kind == ProgressItemKind::DirectoryUnit
                    && (item.index != 0 || plan.len() != 1))
                || !unique.insert(*item)
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressFailureReason {
    NetworkUnavailable,
    TimedOut,
    AccessDenied,
    StorageUnavailable,
    IntegrityCheckFailed,
    Interrupted,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressTerminalOutcome {
    Completed,
    Failed(ProgressFailureReason),
    Cancelled(FileTransferCancellationReason),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressTerminal {
    pub item: ProgressItem,
    pub round: u32,
    pub sequence: u64,
    pub result_revision: u64,
    pub bytes: u64,
    pub total: Option<u64>,
    pub outcome: ProgressTerminalOutcome,
}

#[derive(Default)]
pub struct ProgressTerminalSettlement {
    current: Option<ProgressTerminal>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressTerminalEffect {
    /// 保存此终态后才允许发布变化、回复精确 ACK。
    Adopted,
    /// 已保存完全相同的终态；不重复发布变化，允许回复同一 ACK。
    Duplicate,
    /// 不修改既有事实、不发布变化、不产生 ACK。
    Conflict,
}
impl ProgressTerminalSettlement {
    pub fn terminal(&self) -> Option<ProgressTerminal> {
        self.current
    }
    pub fn apply(&mut self, result: ProgressTerminal) -> ProgressTerminalEffect {
        match self.current {
            None => {
                self.current = Some(result);
                ProgressTerminalEffect::Adopted
            }
            Some(current) if current == result => ProgressTerminalEffect::Duplicate,
            Some(_) => ProgressTerminalEffect::Conflict,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        valid_progress_plan, FileTransferCancellationReason, ProgressFailureReason, ProgressItem,
        ProgressItemKind, ProgressTerminal, ProgressTerminalEffect as Effect,
        ProgressTerminalOutcome as Outcome, ProgressTerminalSettlement, MAX_PROGRESS_ITEMS,
    };
    #[test]
    fn exact_duplicates_are_idempotent_and_every_conflicting_terminal_is_refused() {
        let result = ProgressTerminal {
            item: ProgressItem {
                kind: ProgressItemKind::File,
                index: 0,
            },
            round: 1,
            sequence: 4,
            result_revision: 2,
            bytes: 8,
            total: Some(8),
            outcome: Outcome::Completed,
        };
        let mut state = ProgressTerminalSettlement::default();
        assert_eq!(state.apply(result), Effect::Adopted);
        assert_eq!(state.apply(result), Effect::Duplicate);
        for conflict in [
            ProgressTerminal {
                outcome: Outcome::Failed(ProgressFailureReason::StorageUnavailable),
                ..result
            },
            ProgressTerminal {
                outcome: Outcome::Cancelled(FileTransferCancellationReason::LocalUser),
                ..result
            },
            ProgressTerminal { round: 2, ..result },
            ProgressTerminal {
                sequence: 5,
                ..result
            },
            ProgressTerminal {
                result_revision: 3,
                ..result
            },
            ProgressTerminal { bytes: 7, ..result },
            ProgressTerminal {
                total: None,
                ..result
            },
            ProgressTerminal {
                item: ProgressItem {
                    index: 1,
                    ..result.item
                },
                ..result
            },
        ] {
            assert_eq!(state.apply(conflict), Effect::Conflict);
            assert_eq!(state.apply(result), Effect::Duplicate);
        }
    }
    #[test]
    fn logical_plan_is_bounded_and_directory_is_one_indivisible_result() {
        let file = ProgressItem {
            kind: ProgressItemKind::File,
            index: 0,
        };
        let directory = ProgressItem {
            kind: ProgressItemKind::DirectoryUnit,
            index: 0,
        };
        assert!(!valid_progress_plan(&[]));
        assert!(valid_progress_plan(&[file]));
        assert!(valid_progress_plan(&[directory]));
        assert!(!valid_progress_plan(&[file, file]));
        assert!(!valid_progress_plan(&[file, directory]));
        assert!(!valid_progress_plan(&[ProgressItem {
            index: 1,
            ..directory
        }]));
        let largest: Vec<_> = (0..MAX_PROGRESS_ITEMS as u16)
            .map(|index| ProgressItem { index, ..file })
            .collect();
        assert!(valid_progress_plan(&largest));
        assert!(!valid_progress_plan(&[ProgressItem {
            index: MAX_PROGRESS_ITEMS as u16,
            ..file
        }]));
    }
}
