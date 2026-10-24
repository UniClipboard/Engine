use super::mapping::{ack, bound, descriptor, identity, terminal};
use async_trait::async_trait;
use std::error::Error;
use std::io::{self, ErrorKind};
use tokio::io::AsyncReadExt;
use tokio::time::{timeout_at, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, CurrentProgressTaskPort, ProgressReceiveError, ProgressReceiverPort,
    ProgressTaskDescriptor, StreamLease,
};
use uc_core::file_transfer::ProgressIdentity;
use uc_sync_protocol::transfer_progress::v2::{read_frame, write_frame, Envelope, Message, WireId};

pub struct TransferProgressInbound;
impl TransferProgressInbound {
    /// 原始 deadline 只限制网络，不遗弃已经开始的 Application 采用；实际工作持有同一流许可。
    pub async fn serve(
        stream: StreamLease,
        connection: &ConnectionLease,
        generation: u64,
        deadline: Instant,
        cancel: &CancellationToken,
        owner: &dyn ProgressReceiverPort,
    ) -> Result<(), ProgressReceiveError> {
        let mut exchange = Exchange {
            stream,
            connection,
            deadline,
            cancel,
        };
        let response = match exchange.read().await? {
            Message::Register { source_entry } => {
                let registered = owner
                    .register(
                        &connection.peer(),
                        generation,
                        identity(source_entry),
                        &mut exchange,
                    )
                    .await?;
                bound(registered)
            }
            Message::Terminal {
                binding,
                scope,
                owner_revision,
                result,
            } => {
                exchange.end_request().await?;
                let notice = terminal(binding, scope, owner_revision, result);
                let adopted = owner
                    .adopt_terminal(&connection.peer(), generation, notice)
                    .await?;
                if adopted != notice {
                    return Err(invalid());
                }
                ack(adopted)
            }
            _ => return Err(invalid()),
        };
        exchange.write(response).await?;
        exchange.stream.send.finish().map_err(failed)?;
        Ok(())
    }
}

struct Exchange<'a> {
    stream: StreamLease,
    connection: &'a ConnectionLease,
    deadline: Instant,
    cancel: &'a CancellationToken,
}
impl Exchange<'_> {
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
    async fn read(&mut self) -> Result<Message, ProgressReceiveError> {
        self.authorize().await?;
        tokio::select! {
            biased;
            _ = self.cancel.cancelled() => Err(stopped()),
            result = timeout_at(self.deadline, read_frame(&mut self.stream.receive)) => Ok(result.map_err(failed)?.map_err(failed)?.message),
        }
    }
    async fn write(&mut self, message: Message) -> Result<(), ProgressReceiveError> {
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
    async fn end_request(&mut self) -> Result<(), ProgressReceiveError> {
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
#[async_trait]
impl CurrentProgressTaskPort for Exchange<'_> {
    async fn challenge(
        &mut self,
        nonce: ProgressIdentity,
    ) -> Result<ProgressTaskDescriptor, ProgressReceiveError> {
        self.write(Message::Challenge {
            nonce: WireId(nonce.bytes()),
        })
        .await?;
        let Message::CurrentDescriptor {
            challenge,
            descriptor: current,
        } = self.read().await?
        else {
            return Err(invalid());
        };
        if identity(challenge) != nonce {
            return Err(ProgressReceiveError::Stale);
        }
        self.end_request().await?;
        Ok(descriptor(current))
    }
}
fn failed(source: impl Error + Send + Sync + 'static) -> ProgressReceiveError {
    ProgressReceiveError::Exchange(Box::new(source))
}
fn invalid() -> ProgressReceiveError {
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
