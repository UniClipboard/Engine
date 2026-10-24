use super::super::trace_context::{authenticated_parent, inject_current};
use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use uc_application::deps::{
    ConnectionProtocol, HistoryIncoming, HistoryReply, HistoryWirePort, PeerReceiveStream,
    PeerSendStream,
};
use uc_core::membership::{MembershipHistoryExchangeError, MembershipHistoryMessage};
use uc_observability_contract::diagnostics::{
    describe_operation_failure, DiagnosticErrorType, ObservationContext,
};
use uc_sync_protocol::membership_history::{
    decode_message, decode_request, encode_message, encode_request, read_frame, write_frame,
    MembershipHistoryStatus, MembershipHistoryWireError,
};

pub struct MembershipHistoryWire;
fn wire_failure(source: MembershipHistoryWireError) -> MembershipHistoryExchangeError {
    describe_operation_failure(match &source {
        MembershipHistoryWireError::Io(_) => DiagnosticErrorType::StreamFailed,
        _ => DiagnosticErrorType::DecodeFailed,
    });
    MembershipHistoryExchangeError::transport_from(source)
}
#[async_trait]
impl HistoryWirePort for MembershipHistoryWire {
    async fn read_request(
        &self,
        receive: &mut dyn PeerReceiveStream,
    ) -> Result<HistoryIncoming, MembershipHistoryExchangeError> {
        let request = decode_request(&read_frame(receive).await.map_err(wire_failure)?)
            .map_err(wire_failure)?;
        let observation = authenticated_parent(request.trace_context.as_ref())
            .map(ObservationContext::from_remote_parent)
            .unwrap_or_else(ObservationContext::capture);
        Ok(HistoryIncoming {
            message: request.message,
            observation,
        })
    }
    async fn write_request(
        &self,
        send: &mut dyn PeerSendStream,
        message: MembershipHistoryMessage,
    ) -> Result<(), MembershipHistoryExchangeError> {
        let payload = encode_request(message, inject_current()).map_err(wire_failure)?;
        write_frame(send, &payload).await.map_err(wire_failure)?;
        send.finish()
            .map_err(MembershipHistoryExchangeError::transport_from)?;
        Ok(())
    }
    async fn read_response(
        &self,
        receive: &mut dyn PeerReceiveStream,
        protocol: ConnectionProtocol,
    ) -> Result<HistoryReply, MembershipHistoryExchangeError> {
        let byte = receive
            .read_u8()
            .await
            .map_err(MembershipHistoryExchangeError::transport_from)?;
        let status = MembershipHistoryStatus::decode(byte).map_err(wire_failure)?;
        match status {
            MembershipHistoryStatus::Busy => Err(MembershipHistoryExchangeError::PairingInProgress),
            MembershipHistoryStatus::Rejected => Err(MembershipHistoryExchangeError::Rejected),
            MembershipHistoryStatus::Accepted | MembershipHistoryStatus::AcceptedClosing => {
                if status == MembershipHistoryStatus::AcceptedClosing
                    && protocol != ConnectionProtocol::MultiplexedMembershipHistory
                {
                    return Err(wire_failure(MembershipHistoryWireError::InvalidLayout));
                }
                let message = decode_message(&read_frame(receive).await.map_err(wire_failure)?)
                    .map_err(wire_failure)?;
                Ok(HistoryReply {
                    message,
                    reusable: status == MembershipHistoryStatus::Accepted
                        && protocol == ConnectionProtocol::MultiplexedMembershipHistory,
                })
            }
        }
    }
    async fn respond(
        &self,
        send: &mut dyn PeerSendStream,
        response: Result<MembershipHistoryMessage, MembershipHistoryExchangeError>,
        protocol: ConnectionProtocol,
        reusable: bool,
    ) -> Result<(), MembershipHistoryExchangeError> {
        let (status, payload) = match response {
            Ok(message) => (
                if reusable || protocol == ConnectionProtocol::MembershipHistory {
                    MembershipHistoryStatus::Accepted
                } else {
                    MembershipHistoryStatus::AcceptedClosing
                },
                Some(encode_message(&message).map_err(wire_failure)?),
            ),
            Err(MembershipHistoryExchangeError::PairingInProgress) => {
                (MembershipHistoryStatus::Busy, None)
            }
            Err(_failure) => (MembershipHistoryStatus::Rejected, None),
        };
        send.write_all(&[status as u8])
            .await
            .map_err(MembershipHistoryExchangeError::transport_from)?;
        if let Some(payload) = payload {
            write_frame(send, &payload).await.map_err(wire_failure)?;
        }
        send.finish()
            .map_err(MembershipHistoryExchangeError::transport_from)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, ReadBuf};
    use uc_application::deps::ConnectivityError;
    use uc_core::membership::MembershipHistoryAckV3;

    struct Receive(Cursor<Vec<u8>>);
    impl AsyncRead for Receive {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buffer: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            AsyncRead::poll_read(Pin::new(&mut self.0), cx, buffer)
        }
    }
    impl PeerReceiveStream for Receive {
        fn id(&self) -> u64 {
            0
        }
        fn stop(&mut self, _code: u64) -> Result<(), ConnectivityError> {
            Err(ConnectivityError::Stopped)
        }
    }

    #[tokio::test]
    async fn busy_and_rejected_statuses_keep_the_original_business_classification() {
        let mut busy = Receive(Cursor::new(vec![3]));
        assert!(matches!(
            MembershipHistoryWire
                .read_response(&mut busy, ConnectionProtocol::MembershipHistory)
                .await,
            Err(MembershipHistoryExchangeError::PairingInProgress)
        ));
        let mut rejected = Receive(Cursor::new(vec![2]));
        assert!(matches!(
            MembershipHistoryWire
                .read_response(&mut rejected, ConnectionProtocol::MembershipHistory)
                .await,
            Err(MembershipHistoryExchangeError::Rejected)
        ));
    }
    #[tokio::test]
    async fn closing_hint_is_transport_only_and_forbidden_on_the_legacy_protocol() {
        let bytes = vec![4, 0, 0, 0, 3, 4, 3, 5];
        let mut current = Receive(Cursor::new(bytes.clone()));
        let reply = MembershipHistoryWire
            .read_response(
                &mut current,
                ConnectionProtocol::MultiplexedMembershipHistory,
            )
            .await
            .unwrap();
        assert!(!reply.reusable);
        assert_eq!(
            reply.message,
            MembershipHistoryMessage::AckV3(MembershipHistoryAckV3::Invalid)
        );
        let mut legacy = Receive(Cursor::new(bytes));
        assert!(MembershipHistoryWire
            .read_response(&mut legacy, ConnectionProtocol::MembershipHistory)
            .await
            .is_err());
    }
}
