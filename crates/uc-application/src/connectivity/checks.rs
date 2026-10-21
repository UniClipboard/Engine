//! 准入读取的期限与真实完成权；尚未开始的读取不会因取消而被补启动。
use super::ConnectivityError;
use futures::future::poll_fn;
use std::future::Future;
use std::task::Poll;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;

pub(super) async fn checked(
    work: impl Future<Output = Result<(), ConnectivityError>>,
    supervise: bool,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<(), ConnectivityError> {
    let mut started = false;
    tokio::pin!(work);
    let primary = {
        let polling = poll_fn(|cx| {
            if !started && Instant::now() >= deadline {
                return Poll::Ready(Err(if supervise {
                    ConnectivityError::AdmissionDeadlineExpired
                } else {
                    ConnectivityError::DeadlineExpired
                }));
            }
            started = true;
            work.as_mut().poll(cx)
        });
        tokio::select! {
            biased;
            _ = cancel.cancelled() => ConnectivityError::Stopped,
            result = timeout_at(deadline, polling) => match result {
                Ok(result) => return result,
                Err(source) => if supervise {
                    ConnectivityError::AdmissionTimedOut(source)
                } else { ConnectivityError::TimedOut(source) },
            },
        }
    };
    if supervise && started {
        if let Err(additional) = work.await {
            return Err(ConnectivityError::Settlement {
                primary: Box::new(primary),
                additional: Box::new(additional),
            });
        }
    }
    Err(primary)
}

#[cfg(test)]
#[path = "checks_tests.rs"]
mod tests;
