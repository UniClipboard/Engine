//! 在同一内容身份锁内执行手动重试的最终校验和预期尝试CAS。
use super::{PreparedReceiveRetry, RetryEntryReceiveOutcome};
use crate::clipboard::sync::apply_inbound::ApplyInboundError;
use crate::space::CurrentSpaceMemberScopeError;
use crate::transfer::receive::work::{ReceiveRegistrationError, ReceiveWorkRegistry};
use anyhow::{Context, Error};
use uc_core::ids::EntryId;
use uc_core::ports::{
    AttemptState, BeginReceiveAttemptPort, BeginReceiveOutcome, ClockPort, GetEntryAttemptPort,
};

pub(crate) async fn accept_retry(
    retry: &mut PreparedReceiveRetry,
    work: &ReceiveWorkRegistry,
    get: &dyn GetEntryAttemptPort,
    begin: &dyn BeginReceiveAttemptPort,
    clock: &dyn ClockPort,
) -> Result<Option<String>, ApplyInboundError> {
    let current = get
        .get_entry_attempt(retry.entry.as_ref())
        .await
        .context("read expected receive retry attempt")
        .map_err(ApplyInboundError::Internal)?;
    let rejection = match current {
        None => Some(RetryEntryReceiveOutcome::Unavailable),
        Some(current) if current.current_attempt_id != retry.expected_attempt => {
            Some(RetryEntryReceiveOutcome::Superseded)
        }
        Some(current) if current.state == AttemptState::Cancelling => {
            Some(RetryEntryReceiveOutcome::CancellationIncomplete)
        }
        Some(current) if !current.state.is_terminal() => {
            Some(RetryEntryReceiveOutcome::AlreadyReceiving)
        }
        Some(_) => None,
    };
    if let Some(outcome) = rejection {
        if let Some(reply) = retry.accepted.take() {
            let _ = reply.send(outcome);
        }
        return Ok(None);
    }
    if work.owns_entry(retry.entry.as_ref()) {
        if let Some(reply) = retry.accepted.take() {
            let _ = reply.send(RetryEntryReceiveOutcome::CancellationIncomplete);
        }
        return Ok(None);
    }
    let scope = match retry.scope.snapshot().await {
        Ok(scope) => scope,
        Err(
            CurrentSpaceMemberScopeError::Locked | CurrentSpaceMemberScopeError::NoCurrentSpace,
        ) => {
            if let Some(reply) = retry.accepted.take() {
                let _ = reply.send(RetryEntryReceiveOutcome::AccessDenied);
            }
            return Ok(None);
        }
        Err(source) => {
            return Err(ApplyInboundError::Internal(
                Error::new(source).context("recheck receive retry membership"),
            ))
        }
    };
    if !scope.local_member_active
        || !scope
            .usable_peer_device_ids
            .contains(&retry.source.from_device)
    {
        if let Some(reply) = retry.accepted.take() {
            let _ = reply.send(RetryEntryReceiveOutcome::AccessDenied);
        }
        return Ok(None);
    }
    let identity = retry
        .identity
        .get_entry_snapshot_hash(&retry.entry)
        .await
        .context("recheck original receive content identity")
        .map_err(ApplyInboundError::Internal)?;
    if identity
        .as_ref()
        .is_some_and(|hash| hash != &retry.source.expected_hash)
        || (identity.is_none() && retry.source.original_wire.is_none())
    {
        if let Some(reply) = retry.accepted.take() {
            let _ = reply.send(RetryEntryReceiveOutcome::Unavailable);
        }
        return Ok(None);
    }
    if retry
        .cancellation
        .is_settling(&retry.entry, &retry.expected_attempt)
    {
        if let Some(reply) = retry.accepted.take() {
            let _ = reply.send(RetryEntryReceiveOutcome::CancellationIncomplete);
        }
        return Ok(None);
    }
    let attempt = EntryId::new().to_string();
    if let Err(refusal) = retry.action.register(&retry.entry, &attempt) {
        let outcome = match refusal {
            ReceiveRegistrationError::GateClosed | ReceiveRegistrationError::EntryDeleted => {
                RetryEntryReceiveOutcome::Unavailable
            }
            ReceiveRegistrationError::CleanupInProgress => {
                RetryEntryReceiveOutcome::CancellationIncomplete
            }
        };
        if let Some(reply) = retry.accepted.take() {
            let _ = reply.send(outcome);
        }
        return Ok(None);
    }
    if retry.action.cancellation().is_cancelled() {
        if let Some(reply) = retry.accepted.take() {
            let _ = reply.send(RetryEntryReceiveOutcome::Unavailable);
        }
        return Ok(None);
    }
    let outcome = begin
        .begin_redelivery(
            retry.entry.as_ref(),
            &retry.expected_attempt,
            &attempt,
            clock.now_ms(),
        )
        .await
        .context("accept selected receive retry")
        .map_err(ApplyInboundError::Internal)?;
    let accepted = match outcome {
        BeginReceiveOutcome::Begun => RetryEntryReceiveOutcome::Accepted {
            attempt_id: attempt.clone(),
        },
        BeginReceiveOutcome::AlreadyReceiving => RetryEntryReceiveOutcome::AlreadyReceiving,
        BeginReceiveOutcome::Superseded => RetryEntryReceiveOutcome::Superseded,
    };
    if outcome != BeginReceiveOutcome::Begun {
        if let Some(reply) = retry.accepted.take() {
            let _ = reply.send(accepted);
        }
        return Ok(None);
    }
    Ok(Some(attempt))
}
