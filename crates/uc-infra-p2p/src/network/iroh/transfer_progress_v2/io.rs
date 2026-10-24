use std::error::Error;
use std::io::{self, ErrorKind};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ProgressExchangeDeadline, ProgressReceiveError, StreamLease,
};
use uc_sync_protocol::transfer_progress::v2::{read_frame, write_frame, Envelope, Message};

pub(super) struct Exchange<'a> {
    stream: &'a mut StreamLease,
    connection: &'a ConnectionLease,
    deadline: &'a ProgressExchangeDeadline,
    cancel: &'a CancellationToken,
}
impl<'a> Exchange<'a> {
    pub(super) fn new(
        stream: &'a mut StreamLease,
        connection: &'a ConnectionLease,
        deadline: &'a ProgressExchangeDeadline,
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
        // 准入读取监督至真实结束；原经过时间预算随后禁止过期网络，不丢弃实际本机工作。
        self.connection.check_current().await.map_err(failed)?;
        self.deadline.check(self.cancel)
    }
    pub(super) async fn read(&mut self) -> Result<Message, ProgressReceiveError> {
        self.authorize().await?;
        self.deadline
            .network(
                async {
                    Ok(read_frame(&mut self.stream.receive)
                        .await
                        .map_err(failed)?
                        .message)
                },
                self.cancel,
            )
            .await
    }
    pub(super) async fn write(&mut self, message: Message) -> Result<(), ProgressReceiveError> {
        self.authorize().await?;
        let envelope = Envelope {
            message,
            trace_context: None,
        };
        self.deadline
            .network(
                async {
                    write_frame(&mut self.stream.send, &envelope)
                        .await
                        .map_err(failed)
                },
                self.cancel,
            )
            .await
    }
    pub(super) async fn end_request(&mut self) -> Result<(), ProgressReceiveError> {
        self.authorize().await?;
        self.deadline
            .network(
                async {
                    match self.stream.receive.read_u8().await {
                        Err(error) if error.kind() == ErrorKind::UnexpectedEof => Ok(()),
                        Err(source) => Err(failed(source)),
                        Ok(_) => Err(invalid()),
                    }
                },
                self.cancel,
            )
            .await
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
