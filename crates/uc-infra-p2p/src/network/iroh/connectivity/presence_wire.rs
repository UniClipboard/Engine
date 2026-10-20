//! Uni 半流上的有界 presence 字节交换；不读取资格或维护在线状态。
use async_trait::async_trait;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout_at, Instant};
use uc_application::deps::{
    PeerReachabilityWireError, PeerReachabilityWirePort, PeerReceiveStream, PeerSendStream,
};
use uc_sync_protocol::peer_reachability::{
    self, CheckReply, FrameError, ADMISSION_ACCEPTED, ADMISSION_REJECTED, ADMISSION_REQUEST,
    FRAME_SIZE,
};

pub struct PeerReachabilityWire;
fn step(deadline: Instant) -> Instant {
    deadline.min(Instant::now() + Duration::from_secs(5))
}
async fn read(
    receive: &mut dyn PeerReceiveStream,
    size: usize,
    deadline: Instant,
) -> Result<Vec<u8>, PeerReachabilityWireError> {
    if Instant::now() >= deadline {
        return Err(PeerReachabilityWireError::DeadlineExpired);
    }
    let mut bytes = Vec::with_capacity(size + 1);
    timeout_at(
        step(deadline),
        receive.take((size + 1) as u64).read_to_end(&mut bytes),
    )
    .await
    .map_err(PeerReachabilityWireError::TimedOut)?
    .map_err(PeerReachabilityWireError::Io)?;
    if bytes.len() != size {
        return Err(PeerReachabilityWireError::InvalidFrame(Box::new(
            FrameError::Length,
        )));
    }
    Ok(bytes)
}
async fn write(
    send: &mut dyn PeerSendStream,
    bytes: &[u8],
    deadline: Instant,
) -> Result<(), PeerReachabilityWireError> {
    if Instant::now() >= deadline {
        return Err(PeerReachabilityWireError::DeadlineExpired);
    }
    timeout_at(step(deadline), send.write_all(bytes))
        .await
        .map_err(PeerReachabilityWireError::TimedOut)?
        .map_err(PeerReachabilityWireError::Io)?;
    send.finish().map_err(PeerReachabilityWireError::Connection)
}
#[async_trait]
impl PeerReachabilityWirePort for PeerReachabilityWire {
    async fn read_admission(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        let bytes = read(receive, 1, deadline).await?;
        if bytes[0] != ADMISSION_REQUEST {
            return Err(PeerReachabilityWireError::InvalidFrame(Box::new(
                FrameError::Type,
            )));
        }
        Ok(())
    }
    async fn write_admission(
        &self,
        send: &mut dyn PeerSendStream,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        write(send, &[ADMISSION_REQUEST], deadline).await
    }
    async fn read_confirmation(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<bool, PeerReachabilityWireError> {
        match read(receive, 1, deadline).await?[0] {
            ADMISSION_ACCEPTED => Ok(true),
            ADMISSION_REJECTED => Ok(false),
            _ => Err(PeerReachabilityWireError::InvalidFrame(Box::new(
                FrameError::Type,
            ))),
        }
    }
    async fn write_confirmation(
        &self,
        send: &mut dyn PeerSendStream,
        admitted: bool,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        write(
            send,
            &[if admitted {
                ADMISSION_ACCEPTED
            } else {
                ADMISSION_REJECTED
            }],
            deadline,
        )
        .await?;
        if !admitted {
            // 拒绝后将关闭临时连接，必须先给原 FIN/回执机会，仍受同一网络步骤期限约束。
            timeout_at(step(deadline), send.settled())
                .await
                .map_err(PeerReachabilityWireError::TimedOut)?
                .map_err(PeerReachabilityWireError::Connection)?;
        }
        Ok(())
    }
    async fn read_challenge(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<[u8; 16], PeerReachabilityWireError> {
        peer_reachability::challenge(&read(receive, FRAME_SIZE, deadline).await?)
            .map_err(|source| PeerReachabilityWireError::InvalidFrame(Box::new(source)))
    }
    async fn write_challenge(
        &self,
        send: &mut dyn PeerSendStream,
        nonce: &[u8; 16],
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        write(send, &peer_reachability::request(nonce), deadline).await
    }
    async fn read_reply(
        &self,
        receive: &mut dyn PeerReceiveStream,
        nonce: &[u8; 16],
        deadline: Instant,
    ) -> Result<bool, PeerReachabilityWireError> {
        peer_reachability::classify(&read(receive, FRAME_SIZE, deadline).await?, nonce)
            .map(|reply| reply == CheckReply::Accepted)
            .map_err(|source| PeerReachabilityWireError::InvalidFrame(Box::new(source)))
    }
    async fn write_reply(
        &self,
        send: &mut dyn PeerSendStream,
        nonce: &[u8; 16],
        admitted: bool,
        deadline: Instant,
    ) -> Result<(), PeerReachabilityWireError> {
        write(send, &peer_reachability::reply(nonce, admitted), deadline).await?;
        if !admitted {
            timeout_at(step(deadline), send.settled())
                .await
                .map_err(PeerReachabilityWireError::TimedOut)?
                .map_err(PeerReachabilityWireError::Connection)?;
        }
        Ok(())
    }
}
