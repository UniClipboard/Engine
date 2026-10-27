//! 新挑战到达后才从当前接收负责人取得描述；只做有界交换和精确回显检查。
use super::io::{invalid, Exchange};
use super::mapping::{
    descriptor, identity, outgoing_contact, outgoing_terminal, outgoing_update, terminal,
    wire_descriptor,
};
use async_trait::async_trait;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ProgressBound, ProgressContactNotice, ProgressExchangeDeadline,
    ProgressReceiveError, ProgressTaskAuthorityPort, ProgressTerminalNotice, ProgressUpdateNotice,
    ProgressWirePort, StreamLease,
};
use uc_core::file_transfer::ProgressIdentity;
use uc_sync_protocol::transfer_progress::v2::{Message, WireId};

pub struct TransferProgressWire;
#[async_trait]
impl ProgressWirePort for TransferProgressWire {
    async fn contact(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressContactNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        let mut exchange = Exchange::new(stream, connection, deadline, cancel);
        exchange.write(outgoing_contact(notice)).await?;
        exchange.finish_send().await?;
        exchange.end_request().await?;
        Ok(())
    }

    async fn update(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressUpdateNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<(), ProgressReceiveError> {
        let mut exchange = Exchange::new(stream, connection, deadline, cancel);
        exchange.write(outgoing_update(notice)).await?;
        exchange.finish_send().await?;
        // 只结束本轮传输；对端FIN不是业务终态或文件输出确认。
        exchange.end_request().await
    }
    async fn register(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        authority: &dyn ProgressTaskAuthorityPort,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressBound, ProgressReceiveError> {
        let mut exchange = Exchange::new(stream, connection, deadline, cancel);
        exchange
            .write(Message::Register {
                source_entry: WireId(source.bytes()),
            })
            .await?;
        let Message::Challenge { nonce } = exchange.read().await? else {
            return Err(invalid());
        };
        // 不使用拨号前的预检或排队任务所捕获的descriptor。
        let current = authority
            .current_task(&connection.peer(), source)
            .ok_or(ProgressReceiveError::UnknownTask)?;
        if current.descriptor.source != source {
            return Err(ProgressReceiveError::Stale);
        }
        exchange
            .write(Message::CurrentDescriptor {
                challenge: nonce,
                descriptor: wire_descriptor(current.descriptor),
            })
            .await?;
        exchange.finish_send().await?;
        let Message::Bound {
            challenge,
            binding,
            descriptor: accepted,
        } = exchange.read().await?
        else {
            return Err(invalid());
        };
        if challenge != nonce || descriptor(accepted) != current.descriptor {
            return Err(ProgressReceiveError::Stale);
        }
        exchange.end_request().await?;
        Ok(ProgressBound {
            challenge: identity(challenge),
            binding: identity(binding),
            descriptor: current.descriptor,
        })
    }

    async fn terminal(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        notice: ProgressTerminalNotice,
        deadline: &ProgressExchangeDeadline,
        cancel: &CancellationToken,
    ) -> Result<ProgressTerminalNotice, ProgressReceiveError> {
        let mut exchange = Exchange::new(stream, connection, deadline, cancel);
        exchange.write(outgoing_terminal(notice)).await?;
        exchange.finish_send().await?;
        let Message::TerminalAck {
            binding,
            scope,
            owner_revision,
            result,
        } = exchange.read().await?
        else {
            return Err(invalid());
        };
        let adopted = terminal(binding, scope, owner_revision, result);
        if adopted != notice {
            return Err(ProgressReceiveError::Conflict);
        }
        exchange.end_request().await?;
        Ok(adopted)
    }
}
