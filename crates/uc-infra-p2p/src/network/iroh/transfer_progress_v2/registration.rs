//! 新挑战到达后才从当前接收负责人取得描述；只做有界交换和精确回显检查。
use super::io::{invalid, Exchange};
use super::mapping::{descriptor, identity, wire_descriptor};
use async_trait::async_trait;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ProgressBound, ProgressReceiveError, ProgressRegistrationWirePort,
    ProgressTaskAuthorityPort, StreamLease,
};
use uc_core::file_transfer::ProgressIdentity;
use uc_sync_protocol::transfer_progress::v2::{Message, WireId};

pub struct TransferProgressRegistration;
#[async_trait]
impl ProgressRegistrationWirePort for TransferProgressRegistration {
    async fn register(
        &self,
        stream: &mut StreamLease,
        connection: &ConnectionLease,
        source: ProgressIdentity,
        authority: &dyn ProgressTaskAuthorityPort,
        deadline: Instant,
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
}
