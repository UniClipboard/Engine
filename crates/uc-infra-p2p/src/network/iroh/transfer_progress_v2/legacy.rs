//! 旧单向帧只提交无尝试关联的完整动作；没有业务ACK，也不采用旧字节或终态。
use super::io::{failed, invalid};
use super::TransferProgressInbound;
use std::io::ErrorKind;
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ConnectionLease, ConnectionProtocol, PeerReceiveStream, ProgressExchangeDeadline,
    ProgressReceiveError, ProgressUnattributedReceiverPort, UniStreamLease,
};
use uc_core::file_transfer::ProgressIdentity;
use uc_sync_protocol::transfer_progress::{read_frame, ProgressWireError};

impl TransferProgressInbound {
    /// 整个帧与FIN核验后才交给原投影负责人；已开始的本机采用持有同一工作许可直到结束。
    pub async fn serve_legacy(
        mut stream: UniStreamLease<Box<dyn PeerReceiveStream>>,
        connection: &ConnectionLease,
        deadline: ProgressExchangeDeadline,
        cancel: &CancellationToken,
        owner: &dyn ProgressUnattributedReceiverPort,
    ) -> Result<(), ProgressReceiveError> {
        if connection.protocol() != ConnectionProtocol::TransferProgressV1 {
            return Err(ProgressReceiveError::Protocol);
        }
        connection.check_current().await?;
        deadline.check(cancel)?;
        let source = deadline
            .network(
                async {
                    let frame = read_frame(&mut stream.stream).await.map_err(frame_failed)?;
                    match stream.stream.read_u8().await {
                        Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
                            Ok(ProgressIdentity::new(frame.transfer_id_bytes))
                        }
                        Err(source) => Err(failed(source)),
                        Ok(_) => Err(invalid()),
                    }
                },
                cancel,
            )
            .await?;
        // 完整旧帧不能越过FIN等待期间的撤销、原会话退休、取消或原期限。
        connection.check_current().await?;
        deadline.check(cancel)?;
        owner.observe_unattributed(&connection.peer(), source).await
    }
}

fn frame_failed(source: ProgressWireError) -> ProgressReceiveError {
    match &source {
        ProgressWireError::Io(_) => failed(source),
        _ => ProgressReceiveError::Malformed(Box::new(source)),
    }
}
