use std::error::Error;
use std::io::{self, ErrorKind};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ProgressExchangeDeadline, ProgressReceiveError, StreamLease,
};
use uc_sync_protocol::transfer_progress::v2::{
    read_frame, write_frame, Envelope, Message, WireError, WireId,
};

pub(super) struct Exchange<'a> {
    /// 已实际发出的登记挑战，用于拒绝响应关联；不暴露给业务调用方。
    pub(super) registration_challenge: Option<WireId>,
    request_ended: bool,
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
            registration_challenge: None,
            request_ended: false,
            stream,
            connection,
            deadline,
            cancel,
        }
    }
    pub(super) async fn finish_send(&mut self) -> Result<(), ProgressReceiveError> {
        self.authorize().await?;
        self.stream.send.finish()?;
        Ok(())
    }
    async fn authorize(&mut self) -> Result<(), ProgressReceiveError> {
        // 准入读取监督至真实结束；原经过时间预算随后禁止过期网络，不丢弃实际本机工作。
        self.connection.check_current().await?;
        self.deadline.check(self.cancel)
    }
    pub(super) async fn read(&mut self) -> Result<Message, ProgressReceiveError> {
        self.authorize().await?;
        self.deadline
            .network(
                async {
                    Ok(read_frame(&mut self.stream.receive)
                        .await
                        .map_err(frame_failed)?
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
                        .map_err(frame_failed)
                },
                self.cancel,
            )
            .await
    }
    pub(super) async fn end_request(&mut self) -> Result<(), ProgressReceiveError> {
        self.authorize().await?;
        if self.request_ended {
            return Ok(());
        }
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
            .await?;
        // FIN等待可以跨撤销或会话退休；开始本地采用／查询前重新核对原连接资格。
        self.authorize().await?;
        self.request_ended = true;
        Ok(())
    }
}
pub(super) fn failed(source: impl Error + Send + Sync + 'static) -> ProgressReceiveError {
    ProgressReceiveError::Exchange(Box::new(source))
}
pub(super) fn invalid() -> ProgressReceiveError {
    ProgressReceiveError::Malformed(Box::new(io::Error::new(
        ErrorKind::InvalidData,
        "progress exchange message order invalid",
    )))
}

fn frame_failed(source: WireError) -> ProgressReceiveError {
    match &source {
        WireError::Io(_) => failed(source),
        _ => ProgressReceiveError::Malformed(Box::new(source)),
    }
}

#[cfg(test)]
mod tests {
    use super::{frame_failed, invalid};
    use std::error::Error;
    use std::io;
    use uc_application::deps::ProgressReceiveError;
    use uc_core::error_class::ErrorClass;
    use uc_sync_protocol::transfer_progress::v2::{FrameRefusal, WireError};

    #[test]
    fn malformed_frames_and_message_order_keep_sources_separate_from_temporary_io() {
        let malformed = frame_failed(WireError::Frame(FrameRefusal::Length));
        assert!(matches!(malformed, ProgressReceiveError::Malformed(_)));
        assert_eq!(malformed.class(), "progress_protocol_invalid");
        assert!(matches!(
            malformed.source().unwrap().downcast_ref::<WireError>(),
            Some(WireError::Frame(FrameRefusal::Length))
        ));
        let order = invalid();
        assert_eq!(order.class(), "progress_protocol_invalid");
        assert_eq!(
            order
                .source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::InvalidData
        );
        let transient = frame_failed(WireError::Io(io::ErrorKind::TimedOut.into()));
        assert!(matches!(transient, ProgressReceiveError::Exchange(_)));
        let wire = transient
            .source()
            .unwrap()
            .downcast_ref::<WireError>()
            .unwrap();
        assert_eq!(
            wire.source()
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::TimedOut
        );
    }
}
