use std::error::Error;
use std::io::{self, ErrorKind};
use tokio::io::AsyncReadExt;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{ConnectionLease, ProgressReceiveError, StreamLease};
use uc_sync_protocol::transfer_progress::v2::{read_frame, write_frame, Envelope, Message};

pub(super) struct Exchange<'a> {
    stream: &'a mut StreamLease,
    connection: &'a ConnectionLease,
    deadline: Instant,
    cancel: &'a CancellationToken,
}
impl<'a> Exchange<'a> {
    pub(super) fn new(
        stream: &'a mut StreamLease,
        connection: &'a ConnectionLease,
        deadline: Instant,
        cancel: &'a CancellationToken,
    ) -> Self {
        Self {
            stream,
            connection,
            deadline,
            cancel,
        }
    }
    pub(super) async fn finish_send(&mut self) -> Result<(), ProgressReceiveError> {
        self.authorize().await?;
        self.stream.send.finish().map_err(failed)
    }

    async fn authorize(&mut self) -> Result<(), ProgressReceiveError> {
        // 准入读取监督至真实结束；期限和取消只禁止后续网络，不丢弃读取工作。
        self.connection.check_current().await.map_err(failed)?;
        if self.cancel.is_cancelled() {
            return Err(stopped());
        }
        if Instant::now() >= self.deadline {
            return Err(failed(io::Error::new(
                ErrorKind::TimedOut,
                "progress exchange deadline elapsed",
            )));
        }
        Ok(())
    }
    pub(super) async fn read(&mut self) -> Result<Message, ProgressReceiveError> {
        self.authorize().await?;
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(stopped()),
            result = timeout_at(self.deadline, read_frame(&mut self.stream.receive)) => Ok(result.map_err(failed)?.map_err(failed)?.message),
        }
    }
    pub(super) async fn write(&mut self, message: Message) -> Result<(), ProgressReceiveError> {
        self.authorize().await?;
        let envelope = Envelope {
            message,
            trace_context: None,
        };
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(stopped()),
            result = timeout_at(self.deadline, write_frame(&mut self.stream.send, &envelope)) => result.map_err(failed)?.map_err(failed),
        }
    }
    pub(super) async fn end_request(&mut self) -> Result<(), ProgressReceiveError> {
        self.authorize().await?;
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(stopped()),
            result = timeout_at(self.deadline, self.stream.receive.read_u8()) => match result.map_err(failed)? {
                Err(error) if error.kind() == ErrorKind::UnexpectedEof => Ok(()),
                Err(source) => Err(failed(source)),
                Ok(_) => Err(invalid()),
            },
        }
    }
}
pub(super) fn failed(source: impl Error + Send + Sync + 'static) -> ProgressReceiveError {
    ProgressReceiveError::Exchange(Box::new(source))
}
pub(super) fn invalid() -> ProgressReceiveError {
    failed(io::Error::new(
        ErrorKind::InvalidData,
        "progress exchange message order invalid",
    ))
}
fn stopped() -> ProgressReceiveError {
    failed(io::Error::new(
        ErrorKind::Interrupted,
        "progress exchange stopped",
    ))
}
