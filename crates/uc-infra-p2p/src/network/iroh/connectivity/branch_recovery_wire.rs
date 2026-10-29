//! 成员分支恢复的有界流编码，兼容既有单次交换协议。
use anyhow::{anyhow, Error};
use async_trait::async_trait;
#[cfg(feature = "branch-recovery-fault-tests")]
use std::{env, fs, io::ErrorKind, path::PathBuf, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::time::{timeout_at, Instant};
use uc_application::deps::{
    BranchRecoveryRequest, BranchRecoveryResponse, BranchRecoveryWireError, BranchRecoveryWirePort,
    PeerReceiveStream, PeerSendStream,
};
use uc_sync_protocol::membership_branch_recovery::{
    decode, encode, MembershipBranchRecoveryWireMessage, MAX_RECOVERY_FRAME_SIZE,
};

pub struct BranchRecoveryWire;

async fn write(
    send: &mut dyn PeerSendStream,
    message: MembershipBranchRecoveryWireMessage,
    deadline: Instant,
) -> Result<(), BranchRecoveryWireError> {
    let bytes = encode(&message).map_err(invalid)?;
    let length = u32::try_from(bytes.len()).map_err(invalid)?;
    timeout_at(deadline, async {
        send.write_u32(length).await?;
        send.write_all(&bytes).await?;
        send.finish()?;
        Ok::<(), Error>(())
    })
    .await
    .map_err(unavailable)?
    .map_err(unavailable)
}

async fn read(
    receive: &mut dyn PeerReceiveStream,
    deadline: Instant,
) -> Result<MembershipBranchRecoveryWireMessage, BranchRecoveryWireError> {
    timeout_at(deadline, async {
        let length = receive.read_u32().await.map_err(unavailable)? as usize;
        if length == 0 || length > MAX_RECOVERY_FRAME_SIZE {
            return Err(invalid(anyhow!(
                "membership branch recovery frame size is invalid"
            )));
        }
        let mut bytes = Vec::with_capacity(length);
        receive
            .take((length + 1) as u64)
            .read_to_end(&mut bytes)
            .await
            .map_err(unavailable)?;
        if bytes.len() != length {
            return Err(invalid(anyhow!(
                "membership branch recovery frame length is invalid"
            )));
        }
        decode(&bytes).map_err(invalid)
    })
    .await
    .map_err(unavailable)?
}

#[async_trait]
impl BranchRecoveryWirePort for BranchRecoveryWire {
    async fn write_request(
        &self,
        send: &mut dyn PeerSendStream,
        request: BranchRecoveryRequest,
        deadline: Instant,
    ) -> Result<(), BranchRecoveryWireError> {
        let message = match request {
            BranchRecoveryRequest::GroupInfo {
                conflict_id,
                target_branch_id,
                recipient_member,
            } => MembershipBranchRecoveryWireMessage::request_group_info(
                conflict_id,
                target_branch_id,
                recipient_member,
            ),
            BranchRecoveryRequest::ExternalCommit {
                conflict_id,
                target_branch_id,
                recipient_member,
                external_commit,
            } => {
                #[cfg(feature = "branch-recovery-fault-tests")]
                let external_commit = test_mutate_commit(external_commit, send.id())?;
                MembershipBranchRecoveryWireMessage::submit_external_commit(
                    conflict_id,
                    target_branch_id,
                    recipient_member,
                    external_commit,
                )
            }
        };
        write(send, message, deadline).await
    }

    async fn read_request(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<BranchRecoveryRequest, BranchRecoveryWireError> {
        match read(receive, deadline).await? {
            MembershipBranchRecoveryWireMessage::RequestGroupInfo {
                conflict_id,
                target_branch_id,
                recipient_member,
                ..
            } => Ok(BranchRecoveryRequest::GroupInfo {
                conflict_id,
                target_branch_id,
                recipient_member,
            }),
            MembershipBranchRecoveryWireMessage::SubmitExternalCommit {
                conflict_id,
                target_branch_id,
                recipient_member,
                external_commit,
                ..
            } => Ok(BranchRecoveryRequest::ExternalCommit {
                conflict_id,
                target_branch_id,
                recipient_member,
                external_commit,
            }),
            _ => Err(invalid(anyhow!(
                "unexpected membership branch recovery request"
            ))),
        }
    }

    async fn write_response(
        &self,
        send: &mut dyn PeerSendStream,
        response: BranchRecoveryResponse,
        deadline: Instant,
    ) -> Result<(), BranchRecoveryWireError> {
        #[cfg(feature = "branch-recovery-fault-tests")]
        let gated = matches!(&response, BranchRecoveryResponse::GroupInfo(_))
            && test_wait_before_response().await?;
        let message = match response {
            BranchRecoveryResponse::GroupInfo(group_info) => {
                MembershipBranchRecoveryWireMessage::group_info(group_info)
            }
            BranchRecoveryResponse::RecoveryPackage(package) => {
                MembershipBranchRecoveryWireMessage::recovery_package(package)
            }
            BranchRecoveryResponse::Rejected => MembershipBranchRecoveryWireMessage::rejected(),
        };
        let result = write(send, message, deadline).await;
        #[cfg(feature = "branch-recovery-fault-tests")]
        if gated {
            test_record_response_attempt()?;
        }
        result
    }

    async fn read_response(
        &self,
        receive: &mut dyn PeerReceiveStream,
        deadline: Instant,
    ) -> Result<BranchRecoveryResponse, BranchRecoveryWireError> {
        match read(receive, deadline).await? {
            MembershipBranchRecoveryWireMessage::GroupInfo { group_info, .. } => {
                Ok(BranchRecoveryResponse::GroupInfo(group_info))
            }
            MembershipBranchRecoveryWireMessage::RecoveryPackage { package, .. } => {
                Ok(BranchRecoveryResponse::RecoveryPackage(package))
            }
            MembershipBranchRecoveryWireMessage::Rejected { .. } => {
                #[cfg(feature = "branch-recovery-fault-tests")]
                test_record_rejection(receive.id())?;
                Ok(BranchRecoveryResponse::Rejected)
            }
            _ => Err(invalid(anyhow!(
                "unexpected membership branch recovery response"
            ))),
        }
    }
}

/// 测试专用：已产生真实响应后等待测试释放，不记录响应内容。
#[cfg(feature = "branch-recovery-fault-tests")]
async fn test_wait_before_response() -> Result<bool, BranchRecoveryWireError> {
    let Some(root) = env::var_os("UC_BRANCH_RECOVERY_TEST_GATE") else {
        return Ok(false);
    };
    let root = PathBuf::from(root);
    if !root.join("network_arm").exists() {
        return Ok(false);
    }
    fs::write(root.join("network_reached"), b"ready").map_err(unavailable)?;
    let deadline = Instant::now() + Duration::from_secs(90);
    while !root.join("network_release").exists() {
        if Instant::now() >= deadline {
            return Err(unavailable(anyhow!("test response gate timed out")));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Ok(true)
}

#[cfg(feature = "branch-recovery-fault-tests")]
fn test_record_response_attempt() -> Result<(), BranchRecoveryWireError> {
    let root = env::var_os("UC_BRANCH_RECOVERY_TEST_GATE")
        .ok_or_else(|| unavailable(anyhow!("test response gate missing")))?;
    fs::write(PathBuf::from(root).join("network_attempted"), b"attempted").map_err(unavailable)
}

/// 只在显式测试构建中注入不同commit，不记录请求内容。
#[cfg(feature = "branch-recovery-fault-tests")]
fn test_mutate_commit(
    mut commit: Vec<u8>,
    stream_id: u64,
) -> Result<Vec<u8>, BranchRecoveryWireError> {
    let Some(root) = env::var_os("UC_BRANCH_RECOVERY_TEST_GATE") else {
        return Ok(commit);
    };
    let root = PathBuf::from(root);
    if root.join("mutate_commit").exists() {
        let first = commit
            .first_mut()
            .ok_or_else(|| invalid(anyhow!("empty test commit")))?;
        *first ^= 1;
        fs::write(root.join("mutated"), stream_id.to_string()).map_err(unavailable)?;
    }
    Ok(commit)
}

#[cfg(feature = "branch-recovery-fault-tests")]
fn test_record_rejection(stream_id: u64) -> Result<(), BranchRecoveryWireError> {
    let Some(root) = env::var_os("UC_BRANCH_RECOVERY_TEST_GATE") else {
        return Ok(());
    };
    let root = PathBuf::from(root);
    if !root.join("mutate_commit").exists() {
        return Ok(());
    }
    let saved = match fs::read_to_string(root.join("mutated")) {
        Ok(saved) => saved,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(unavailable(error)),
    };
    if saved == stream_id.to_string() {
        fs::write(root.join("rejected"), b"rejected").map_err(unavailable)?;
    }
    Ok(())
}

fn unavailable(source: impl Into<Error>) -> BranchRecoveryWireError {
    BranchRecoveryWireError::Unavailable {
        source: source.into(),
    }
}

fn invalid(source: impl Into<Error>) -> BranchRecoveryWireError {
    BranchRecoveryWireError::Invalid {
        source: source.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
        time::Duration,
    };

    use tokio::io::{AsyncRead, AsyncWriteExt, DuplexStream, ReadBuf};
    use tokio::time::Instant;
    use uc_application::deps::{BranchRecoveryWireError, ConnectivityError, PeerReceiveStream};
    use uc_sync_protocol::membership_branch_recovery::{
        encode, MembershipBranchRecoveryWireMessage, MAX_RECOVERY_FRAME_SIZE,
    };

    use super::read;

    struct Receive(DuplexStream);
    impl AsyncRead for Receive {
        fn poll_read(
            mut self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Pin::new(&mut self.0).poll_read(cx, buf)
        }
    }
    impl PeerReceiveStream for Receive {
        fn id(&self) -> u64 {
            1
        }
        fn stop(&mut self, _: u64) -> Result<(), ConnectivityError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn original_frame_with_fin_decodes_and_short_or_extended_frame_is_invalid() {
        let body = encode(&MembershipBranchRecoveryWireMessage::rejected()).unwrap();
        for extra in [-1i32, 0, 1] {
            let (mut send, receive) = tokio::io::duplex(128);
            let length = (body.len() as i32 + extra) as u32;
            send.write_u32(length).await.unwrap();
            send.write_all(&body).await.unwrap();
            send.shutdown().await.unwrap();
            let result = read(
                &mut Receive(receive),
                Instant::now() + Duration::from_secs(1),
            )
            .await;
            if extra == 0 {
                assert!(matches!(
                    result,
                    Ok(MembershipBranchRecoveryWireMessage::Rejected { .. })
                ));
            } else {
                assert!(matches!(
                    result,
                    Err(BranchRecoveryWireError::Invalid { .. })
                ));
            }
        }
    }

    #[tokio::test]
    async fn oversized_length_fails_before_reading_body() {
        let (mut send, receive) = tokio::io::duplex(128);
        send.write_u32((MAX_RECOVERY_FRAME_SIZE + 1) as u32)
            .await
            .unwrap();
        let result = read(
            &mut Receive(receive),
            Instant::now() + Duration::from_secs(1),
        )
        .await;
        assert!(matches!(
            result,
            Err(BranchRecoveryWireError::Invalid { .. })
        ));
    }

    #[tokio::test]
    async fn missing_fin_does_not_outlive_the_original_deadline() {
        let body = encode(&MembershipBranchRecoveryWireMessage::rejected()).unwrap();
        let (mut send, receive) = tokio::io::duplex(128);
        send.write_u32(body.len() as u32).await.unwrap();
        send.write_all(&body).await.unwrap();
        let result = read(
            &mut Receive(receive),
            Instant::now() + Duration::from_millis(30),
        )
        .await;
        assert!(matches!(
            result,
            Err(BranchRecoveryWireError::Unavailable { .. })
        ));
    }
}
