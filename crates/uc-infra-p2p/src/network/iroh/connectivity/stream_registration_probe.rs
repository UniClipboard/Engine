//! 阶段 0 主机原型：真实双端／受管流／生产 Core 围栏；不接生产 reporter 或 Router。
//! ReceiverAuthority 模拟完整接收负责人交接的当前描述，不冒充持久 attempt 查询。
use super::progress_probe::Fixture;
use anyhow::{bail, ensure};
use std::io::ErrorKind;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::time::{timeout, Instant};
use uc_application::deps::{ConnectionMode, ConnectionProtocol, StreamLease};
use uc_core::file_transfer::{
    ProgressRegistration, RegistrationDescriptor, RegistrationEffect as Effect,
    RegistrationInput as Input, RegistrationRefusal as Refusal,
};
use uc_sync_protocol::transfer_progress::v2::{
    read_frame, write_frame, ActiveProgress, Envelope, ItemKind, LogicalItem, Message, Phase,
    ProducerDescriptor, RejectionReason, TaskScope, TerminalOutcome, TerminalResult, WireId,
    TRANSFER_PROGRESS_ALPN,
};

const BUDGET: Duration = Duration::from_secs(5);
const SOURCE: WireId = WireId([1; 16]);

struct ReceiverAuthority {
    producer: WireId,
    revision: u64,
    attempt: WireId,
}
impl ReceiverAuthority {
    fn current(&self) -> ProducerDescriptor {
        ProducerDescriptor {
            producer: self.producer,
            owner_revision: self.revision,
            scope: TaskScope {
                source_entry: SOURCE,
                attempt: self.attempt,
            },
            item_count: 3,
        }
    }
}

struct Exchange {
    receiver: StreamLease,
    source: StreamLease,
    nonce: WireId,
    generation: u64,
}

// 唯一外部输入为流所属连接的本地世代；线上帧无法自行指定它。
async fn challenge(
    fixture: &Fixture,
    owner: &mut ProgressRegistration<WireId>,
    generation: u64,
    nonce: WireId,
) -> anyhow::Result<Exchange> {
    let mut receiver = fixture.sending.open_bi(Instant::now() + BUDGET).await?;
    write_frame(
        &mut receiver.send,
        &Envelope {
            message: Message::Register {
                source_entry: SOURCE,
            },
            trace_context: None,
        },
    )
    .await?;
    let mut source = fixture.receiving.accept_bi().await?;
    assert_eq!(
        read_frame(&mut source.receive).await?.message,
        Message::Register {
            source_entry: SOURCE
        }
    );
    assert_eq!(
        owner.apply(Input::Challenge { generation, nonce }),
        Effect::SendChallenge
    );
    write_frame(
        &mut source.send,
        &Envelope {
            message: Message::Challenge { nonce },
            trace_context: None,
        },
    )
    .await?;
    assert_eq!(
        read_frame(&mut receiver.receive).await?.message,
        Message::Challenge { nonce }
    );
    Ok(Exchange {
        receiver,
        source,
        nonce,
        generation,
    })
}

// 完整交换在同一流中按顺序读写，明确检查双方 FIN；测试不把 FIN 当业务采用。
async fn describe(
    mut exchange: Exchange,
    owner: &mut ProgressRegistration<WireId>,
    descriptor: ProducerDescriptor,
    binding: WireId,
    expected: Effect,
) -> anyhow::Result<()> {
    write_frame(
        &mut exchange.receiver.send,
        &Envelope {
            message: Message::CurrentDescriptor {
                challenge: exchange.nonce,
                descriptor: descriptor.clone(),
            },
            trace_context: None,
        },
    )
    .await?;
    exchange.receiver.send.finish()?;
    let Message::CurrentDescriptor {
        challenge,
        descriptor,
    } = read_frame(&mut exchange.source.receive).await?.message
    else {
        bail!("expected descriptor")
    };
    ensure!(
        exchange
            .source
            .receive
            .read_u8()
            .await
            .is_err_and(|error| error.kind() == ErrorKind::UnexpectedEof),
        "descriptor requires FIN"
    );
    let actual = owner.apply(Input::Describe {
        generation: exchange.generation,
        nonce: challenge,
        binding,
        descriptor: RegistrationDescriptor {
            producer: descriptor.producer,
            revision: descriptor.owner_revision,
            source: descriptor.scope.source_entry,
            attempt: descriptor.scope.attempt,
            item_count: descriptor.item_count,
        },
    });
    assert_eq!(actual, expected);
    let response = if actual == Effect::SendBound {
        Message::Bound {
            challenge,
            binding,
            descriptor,
        }
    } else {
        Message::Rejected {
            challenge,
            reason: RejectionReason::Stale,
        }
    };
    write_frame(
        &mut exchange.source.send,
        &Envelope {
            message: response.clone(),
            trace_context: None,
        },
    )
    .await?;
    exchange.source.send.finish()?;
    assert_eq!(
        read_frame(&mut exchange.receiver.receive).await?.message,
        response
    );
    ensure!(
        exchange
            .receiver
            .receive
            .read_u8()
            .await
            .is_err_and(|error| error.kind() == ErrorKind::UnexpectedEof),
        "response requires FIN"
    );
    Ok(())
}

async fn notification(
    fixture: &Fixture,
    owner: &mut ProgressRegistration<WireId>,
    generation: u64,
    message: Message,
) -> anyhow::Result<Effect> {
    let mut receiver = fixture.sending.open_bi(Instant::now() + BUDGET).await?;
    write_frame(
        &mut receiver.send,
        &Envelope {
            message,
            trace_context: None,
        },
    )
    .await?;
    receiver.send.finish()?;
    let mut source = fixture.receiving.accept_bi().await?;
    let (binding, scope, revision) = match read_frame(&mut source.receive).await?.message {
        Message::Update {
            binding,
            scope,
            owner_revision,
            ..
        }
        | Message::Terminal {
            binding,
            scope,
            owner_revision,
            ..
        } => (binding, scope, owner_revision),
        _ => bail!("expected notification"),
    };
    ensure!(
        source
            .receive
            .read_u8()
            .await
            .is_err_and(|error| error.kind() == ErrorKind::UnexpectedEof),
        "notification requires FIN"
    );
    let effect = owner.apply(Input::Notification {
        generation,
        binding,
        source: scope.source_entry,
        attempt: scope.attempt,
        revision,
    });
    // 本任务不实现终态采用或 ACK；空响应仅结束测试流。
    source.send.finish()?;
    ensure!(
        receiver
            .receive
            .read_u8()
            .await
            .is_err_and(|error| error.kind() == ErrorKind::UnexpectedEof),
        "test exchange requires FIN"
    );
    Ok(effect)
}

fn update(descriptor: &ProducerDescriptor, binding: WireId) -> Message {
    Message::Update {
        binding,
        scope: descriptor.scope.clone(),
        owner_revision: descriptor.owner_revision,
        progress: ActiveProgress {
            item: LogicalItem {
                kind: ItemKind::File,
                index: 0,
            },
            round: 0,
            sequence: 1,
            bytes: 4,
            total: Some(8),
            phase: Phase::Transferring,
        },
    }
}
fn terminal(descriptor: &ProducerDescriptor, binding: WireId) -> Message {
    Message::Terminal {
        binding,
        scope: descriptor.scope.clone(),
        owner_revision: descriptor.owner_revision,
        result: TerminalResult {
            item: LogicalItem {
                kind: ItemKind::File,
                index: 0,
            },
            round: 0,
            sequence: 2,
            result_revision: 1,
            bytes: 8,
            total: Some(8),
            outcome: TerminalOutcome::Completed,
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delayed_registration_and_old_attempt_notifications_cannot_replace_current(
) -> anyhow::Result<()> {
    timeout(BUDGET, async {
        let fixture = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        assert_eq!(fixture.sending.mode(), ConnectionMode::Multiplexed);
        assert_eq!(
            fixture.sending.protocol(),
            ConnectionProtocol::TransferProgressV2
        );
        let mut owner = ProgressRegistration::new(SOURCE);
        assert_eq!(
            owner.apply(Input::ReplaceConnection { generation: 1 }),
            Effect::ConnectionReplaced
        );
        let mut receiver = ReceiverAuthority {
            producer: WireId([2; 16]),
            revision: 8,
            attempt: WireId([3; 16]),
        };
        let delayed = challenge(&fixture, &mut owner, 1, WireId([10; 16])).await?;
        let old_descriptor = receiver.current();
        // 旧交换保留真实流；新用户尝试先完成登记，无睡眠或任务调度巧合。
        receiver.revision = 9;
        receiver.attempt = WireId([4; 16]);
        let current = challenge(&fixture, &mut owner, 1, WireId([11; 16])).await?;
        let descriptor = receiver.current();
        describe(
            current,
            &mut owner,
            descriptor.clone(),
            WireId([21; 16]),
            Effect::SendBound,
        )
        .await?;
        describe(
            delayed,
            &mut owner,
            old_descriptor.clone(),
            WireId([20; 16]),
            Effect::Refused(Refusal::StaleChallenge),
        )
        .await?;
        for message in [
            update(&old_descriptor, WireId([20; 16])),
            terminal(&old_descriptor, WireId([20; 16])),
        ] {
            assert_eq!(
                notification(&fixture, &mut owner, 1, message).await?,
                Effect::Refused(Refusal::WrongBinding)
            );
        }
        assert_eq!(
            notification(
                &fixture,
                &mut owner,
                1,
                update(&descriptor, WireId([21; 16]))
            )
            .await?,
            Effect::CheckNotification
        );
        fixture.close().await
    })
    .await?
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn physical_reconnect_preserves_revision_and_restart_retires_queued_old_producer(
) -> anyhow::Result<()> {
    timeout(BUDGET, async {
        let original = Fixture::new(4, 4, TRANSFER_PROGRESS_ALPN).await?;
        let mut owner = ProgressRegistration::new(SOURCE);
        owner.apply(Input::ReplaceConnection { generation: 1 });
        let mut receiver = ReceiverAuthority {
            producer: WireId([2; 16]),
            revision: 8,
            attempt: WireId([3; 16]),
        };
        let first = challenge(&original, &mut owner, 1, WireId([10; 16])).await?;
        describe(
            first,
            &mut owner,
            receiver.current(),
            WireId([20; 16]),
            Effect::SendBound,
        )
        .await?;
        let delayed = challenge(&original, &mut owner, 1, WireId([11; 16])).await?;
        let old_descriptor = receiver.current();
        // 复用同两个 Endpoint（身份不变）建立第二条真实 QUIC 连接，不重建 Core 围栏。
        let reconnect = Fixture::connect(
            original.client.clone(),
            original.server.clone(),
            4,
            4,
            TRANSFER_PROGRESS_ALPN,
        )
        .await?;
        owner.apply(Input::ReplaceConnection { generation: 2 });
        let rollback = challenge(&reconnect, &mut owner, 2, WireId([12; 16])).await?;
        receiver.revision = 7;
        describe(
            rollback,
            &mut owner,
            receiver.current(),
            WireId([22; 16]),
            Effect::Refused(Refusal::RevisionRegressed),
        )
        .await?;
        receiver.revision = 8;
        let current = challenge(&reconnect, &mut owner, 2, WireId([13; 16])).await?;
        describe(
            current,
            &mut owner,
            receiver.current(),
            WireId([23; 16]),
            Effect::SendBound,
        )
        .await?;
        let changed_on_same_connection =
            challenge(&reconnect, &mut owner, 2, WireId([15; 16])).await?;
        let unexpected_producer = ReceiverAuthority {
            producer: WireId([5; 16]),
            revision: 1,
            attempt: WireId([6; 16]),
        };
        describe(
            changed_on_same_connection,
            &mut owner,
            unexpected_producer.current(),
            WireId([25; 16]),
            Effect::Refused(Refusal::ProducerChangedOnConnection),
        )
        .await?;
        // 新运行实例修订可从较小值开始，但必须换连接；保留旧流以实际投递迟到回复。
        let restarted = Fixture::connect(
            original.client.clone(),
            original.server.clone(),
            4,
            4,
            TRANSFER_PROGRESS_ALPN,
        )
        .await?;
        owner.apply(Input::ReplaceConnection { generation: 3 });
        receiver.producer = WireId([5; 16]);
        receiver.revision = 1;
        receiver.attempt = WireId([6; 16]);
        let current = challenge(&restarted, &mut owner, 3, WireId([14; 16])).await?;
        let descriptor = receiver.current();
        describe(
            current,
            &mut owner,
            descriptor.clone(),
            WireId([24; 16]),
            Effect::SendBound,
        )
        .await?;
        describe(
            delayed,
            &mut owner,
            old_descriptor.clone(),
            WireId([21; 16]),
            Effect::Refused(Refusal::RetiredConnection),
        )
        .await?;
        assert_eq!(
            notification(
                &original,
                &mut owner,
                1,
                terminal(&old_descriptor, WireId([20; 16]))
            )
            .await?,
            Effect::Refused(Refusal::RetiredConnection)
        );
        assert_eq!(
            notification(
                &restarted,
                &mut owner,
                3,
                update(&descriptor, WireId([24; 16]))
            )
            .await?,
            Effect::CheckNotification
        );
        reconnect.close().await?;
        restarted.close().await?;
        original.close().await
    })
    .await?
}
