//! 原下载持有者的本地结算信号；观察者不持有许可或内容保护。
use anyhow::Error as SourceError;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::watch;
use uc_core::file_transfer::{
    ReceiveFileSettlement, ReceiveFileSettlementEffect, ReceiveFileSettlementInput,
};

#[derive(Debug, Clone, Copy)]
pub(in super::super) enum LocalSettlementOutcome {
    Committed,
    Cancelled,
}
type Completion = Option<Result<LocalSettlementOutcome, Arc<SourceError>>>;

#[derive(Clone)]
pub(in super::super) struct LocalSettlement(Arc<Control>);
struct Control {
    state: Mutex<ReceiveFileSettlement>,
    completion: watch::Sender<Completion>,
}
impl Default for LocalSettlement {
    fn default() -> Self {
        let (completion, _) = watch::channel(None);
        Self(Arc::new(Control {
            state: Mutex::new(ReceiveFileSettlement::default()),
            completion,
        }))
    }
}
impl LocalSettlement {
    pub(in super::super) fn begin_commit(&self) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .apply(ReceiveFileSettlementInput::BeginCommit)
            == ReceiveFileSettlementEffect::Commit
    }
    pub(in super::super) fn cancel(&self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .apply(ReceiveFileSettlementInput::Cancel);
    }
    pub(in super::super) fn is_pending(&self) -> bool {
        self.0.completion.borrow().is_none()
    }
    pub(in super::super) fn result(&self) -> Completion {
        self.0.completion.borrow().clone()
    }
    pub(in super::super) fn is_committed(&self) -> bool {
        matches!(
            self.0.completion.borrow().as_ref(),
            Some(Ok(LocalSettlementOutcome::Committed))
        )
    }
    pub(in super::super) fn finish(
        &self,
        result: Result<LocalSettlementOutcome, Arc<SourceError>>,
    ) {
        let input = match &result {
            Ok(LocalSettlementOutcome::Committed) => ReceiveFileSettlementInput::Committed,
            Ok(LocalSettlementOutcome::Cancelled) => ReceiveFileSettlementInput::Cancelled,
            Err(_) => ReceiveFileSettlementInput::Failed,
        };
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.apply(input) == ReceiveFileSettlementEffect::Settled {
            self.0.completion.send_replace(Some(result));
        }
    }
    pub(in super::super) async fn wait(&self) -> Result<LocalSettlementOutcome, Arc<SourceError>> {
        let mut completion = self.0.completion.subscribe();
        loop {
            if let Some(result) = completion.borrow_and_update().clone() {
                return result;
            }
            completion
                .changed()
                .await
                .map_err(|source| Arc::new(SourceError::new(source)))?;
        }
    }
    pub(in super::super) fn abandon(&self, source: Option<Arc<SourceError>>) {
        if self.is_pending() {
            self.finish(Err(source.unwrap_or_else(|| {
                Arc::new(SourceError::new(LocalSettlementAbandoned))
            })));
        }
    }
}
#[derive(Debug, thiserror::Error)]
#[error("receive file owner exited before local settlement")]
struct LocalSettlementAbandoned;

#[derive(Debug, Clone)]
pub(in super::super) struct LocalSettlementFailure(pub(in super::super) Arc<SourceError>);
impl Display for LocalSettlementFailure {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.write_str("receive file local settlement failed")
    }
}
impl Error for LocalSettlementFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref().as_ref())
    }
}
