use super::io::{invalid, Exchange};
use super::mapping::{ack, bound, contact, descriptor, identity, terminal, update};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, CurrentProgressTaskPort, ProgressExchangeDeadline, ProgressReceiveError,
    ProgressReceiverPort, ProgressTaskDescriptor, StreamLease,
};
use uc_core::file_transfer::ProgressIdentity;
use uc_sync_protocol::transfer_progress::v2::{Message, WireId};

pub struct TransferProgressInbound;
impl TransferProgressInbound {
    /// 原始 deadline 只限制网络，不遗弃已经开始的 Application 采用；实际工作持有同一流许可。
    pub async fn serve(
        mut stream: StreamLease,
        connection: &ConnectionLease,
        generation: u64,
        deadline: ProgressExchangeDeadline,
        cancel: &CancellationToken,
        owner: &dyn ProgressReceiverPort,
    ) -> Result<(), ProgressReceiveError> {
        let mut exchange = Exchange::new(&mut stream, connection, &deadline, cancel);
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
                Some(bound(registered))
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
                Some(ack(adopted))
            }
            Message::Contact {
                binding,
                scope,
                owner_revision,
                sequence,
            } => {
                exchange.end_request().await?;
                owner
                    .adopt_contact(
                        &connection.peer(),
                        generation,
                        contact(binding, scope, owner_revision, sequence),
                    )
                    .await?;
                None
            }
            Message::Update {
                binding,
                scope,
                owner_revision,
                progress,
            } => {
                exchange.end_request().await?;
                owner
                    .adopt_update(
                        &connection.peer(),
                        generation,
                        update(binding, scope, owner_revision, progress),
                    )
                    .await?;
                None
            }
            _ => return Err(invalid()),
        };
        if let Some(response) = response {
            exchange.write(response).await?;
        }
        exchange.finish_send().await?;
        Ok(())
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
