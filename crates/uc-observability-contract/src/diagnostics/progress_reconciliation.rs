//! 用户明确核对的固定展示结果；核对完成不代表文件交付成功。
use super::{
    complete_operation, CompletionResult, DiagnosticDomain, DiagnosticErrorType,
    DiagnosticOperation, DiagnosticRole, OperationCompletion,
};
use crate::wire_enum;
use std::time::Duration;

wire_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum ProgressReconciliationOutcome: &'static str {
        Running => "clipboard.reconcile_progress.running",
        Settled => "clipboard.reconcile_progress.settled",
        Superseded => "clipboard.reconcile_progress.superseded",
        NotRetained => "clipboard.reconcile_progress.not_retained",
        Unsupported => "clipboard.reconcile_progress.unsupported",
        Unreachable => "clipboard.reconcile_progress.unreachable",
        Busy => "clipboard.reconcile_progress.busy",
        AccessDenied => "clipboard.reconcile_progress.access_denied",
        InvalidInput => "clipboard.reconcile_progress.invalid_input",
        InvalidData => "clipboard.reconcile_progress.invalid_data",
        Unavailable => "clipboard.reconcile_progress.unavailable",
        Cancelled => "clipboard.reconcile_progress.cancelled",
    }
}

impl ProgressReconciliationOutcome {
    pub(super) fn approved_name(name: &str) -> bool {
        [
            Self::Running,
            Self::Settled,
            Self::Superseded,
            Self::NotRetained,
            Self::Unsupported,
            Self::Unreachable,
            Self::Busy,
            Self::AccessDenied,
            Self::InvalidInput,
            Self::InvalidData,
            Self::Unavailable,
            Self::Cancelled,
        ]
        .iter()
        .any(|outcome| name == outcome.wire())
    }

    /// 只使用完整能力返回的固定分类；不读取身份、内容或业务内部阶段。
    pub fn complete(self, duration: Duration) {
        tracing::Span::current().record("uc.display.name", self.wire());
        let result = match self {
            Self::Settled => CompletionResult::Succeeded,
            Self::Superseded => CompletionResult::Skipped,
            Self::Running
            | Self::NotRetained
            | Self::Unsupported
            | Self::Unreachable
            | Self::Busy => CompletionResult::Deferred,
            Self::AccessDenied | Self::InvalidInput => CompletionResult::Rejected,
            Self::InvalidData => CompletionResult::Failed(DiagnosticErrorType::Corrupt),
            Self::Unavailable => CompletionResult::Failed(DiagnosticErrorType::Unavailable),
            Self::Cancelled => CompletionResult::Cancelled,
        };
        complete_operation(OperationCompletion {
            domain: DiagnosticDomain::Clipboard,
            operation: DiagnosticOperation::ClipboardReconcileProgress,
            role: DiagnosticRole::Local,
            result,
            duration,
        });
    }
}
