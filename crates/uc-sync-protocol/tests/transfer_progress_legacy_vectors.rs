//! 两个固定旧端（5cbca8fc 与 f6f305d9）的 /1 codec 字节完全相同。
//! 原文件 SHA-256 为 4f8130692441496a436be085f09691cd60236b9c3cc509803a56c504c6d61d0b；
//! 固定全部状态的已知／未知总量向量，不能通过同一实现的往返掩盖字节漂移。

use tokio::io::{duplex, AsyncReadExt};
use uc_core::file_transfer::{FileTransferCancellationReason, OutboundProgressStatus};
use uc_sync_protocol::transfer_progress::{
    read_frame, write_frame, ProgressFrame, FRAME_LEN, LEGACY_TRANSFER_PROGRESS_ALPN,
};

#[tokio::test]
async fn frozen_v1_vectors_encode_and_decode_all_statuses_and_total_forms() {
    let statuses = [
        OutboundProgressStatus::InProgress,
        OutboundProgressStatus::Completed,
        OutboundProgressStatus::Failed,
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::LocalUser,
        },
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::RemotePeer,
        },
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::Replaced,
        },
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::Timeout,
        },
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::Unknown,
        },
        OutboundProgressStatus::Cancelled {
            reason: FileTransferCancellationReason::ConnectivityRecovery,
        },
    ];
    let vectors: Vec<_> = include_str!("fixtures/transfer-progress-v1.txt")
        .lines()
        .collect();
    assert_eq!(vectors.len(), 18);
    assert_eq!(
        LEGACY_TRANSFER_PROGRESS_ALPN,
        b"uniclipboard/transfer-progress/1"
    );
    let mut index = 0;
    for status in statuses {
        for total in [None, Some(4096)] {
            assert_eq!(vectors[index].len(), FRAME_LEN * 2);
            let frame = ProgressFrame {
                transfer_id_bytes: *uuid::Uuid::parse_str("11111111-2222-4333-8444-555555555555")
                    .unwrap()
                    .as_bytes(),
                bytes_transferred: 1024,
                total_bytes: total,
                status,
            };
            let expected: Vec<u8> = vectors[index]
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                .collect();
            assert_eq!(expected.len(), FRAME_LEN);
            let (mut writer, mut reader) = duplex(128);
            write_frame(&mut writer, &frame).await.unwrap();
            drop(writer);
            let mut encoded = Vec::new();
            reader.read_to_end(&mut encoded).await.unwrap();
            assert_eq!(encoded, expected, "vector {index} encode");
            let decoded = read_frame(&mut expected.as_slice()).await.unwrap();
            assert_eq!(decoded, frame, "vector {index} decode");
            index += 1;
        }
    }
}
