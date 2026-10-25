//! 读取实际接收用例失败清理后的结果，再经过真实受管Query与完整分页读取。
use super::super::profile_probe::ProfilePair;
use super::{decode_item, decode_task, progress_budget, Authority, AuthorityProcess, SOURCE};
use crate::network::iroh::transfer_progress_v2::{TransferProgressInbound, TransferProgressWire};
use anyhow::ensure;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{timeout, Instant};
use tokio_util::sync::CancellationToken;
use uc_application::deps::{
    ProgressBound, ProgressExchangeService, ProgressPageReply, ProgressProjectionKey,
    ProgressProjectionRegistry, ProgressReceiveError, ProgressSnapshotPage,
    ProgressSnapshotProviderPort, ProgressSnapshotQuery, ProgressSnapshotRead, ProgressWirePort,
    TransferProgressProducer, TransferProgressReconciler, TransferProgressSnapshotReader,
};
use uc_core::file_transfer::{
    ProgressFailureReason, ProgressFreshnessView, ProgressItemState, ProgressTerminal,
    ProgressTerminalOutcome,
};
use uc_core::ids::DeviceId;
use uc_infra_local::SystemElapsedClock;

pub(super) struct SettledSnapshot(pub(super) Arc<Authority>);
#[async_trait]
impl ProgressSnapshotProviderPort for SettledSnapshot {
    async fn snapshot_page(
        &self,
        peer: &DeviceId,
        query: ProgressSnapshotQuery,
    ) -> Result<ProgressPageReply, ProgressReceiveError> {
        let page = self
            .0
            .request(json!({"op":"snapshot", "peer":peer.as_str(),
                "source":query.source.bytes(),"attempt":query.attempt.map(|attempt|attempt.bytes()),
                "offset":query.offset,"revision":query.revision,
            }))
            .map_err(|source| {
                ProgressReceiveError::Exchange(
                    source.context("read actual settled query fixture").into(),
                )
            })?;
        if page["not_retained"] == true {
            return Ok(ProgressPageReply::NotRetained);
        }
        let current = decode_task(page["task"].clone()).unwrap();
        let items = page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                assert_ne!(
                    item["active"], true,
                    "fixture must use actual settled receive results"
                );
                let reason = match item["failed"].as_str().unwrap() {
                    "NetworkUnavailable" => ProgressFailureReason::NetworkUnavailable,
                    "TimedOut" => ProgressFailureReason::TimedOut,
                    "AccessDenied" => ProgressFailureReason::AccessDenied,
                    "StorageUnavailable" => ProgressFailureReason::StorageUnavailable,
                    "IntegrityCheckFailed" => ProgressFailureReason::IntegrityCheckFailed,
                    "Interrupted" => ProgressFailureReason::Interrupted,
                    "Unknown" => ProgressFailureReason::Unknown,
                    _ => panic!("actual query fixture returned an unknown reason"),
                };
                ProgressItemState::Terminal(ProgressTerminal {
                    item: decode_item(item),
                    round: item["round"].as_u64().unwrap() as u32,
                    sequence: item["sequence"].as_u64().unwrap(),
                    result_revision: item["result_revision"].as_u64().unwrap(),
                    bytes: item["bytes"].as_u64().unwrap(),
                    total: item["total"].as_u64(),
                    outcome: ProgressTerminalOutcome::Failed(reason),
                })
            })
            .collect();
        Ok(ProgressPageReply::Current(ProgressSnapshotPage {
            descriptor: current.descriptor,
            revision: page["revision"].as_u64().unwrap(),
            offset: query.offset,
            total: current.descriptor.item_count,
            items,
        }))
    }
}
async fn register_current(
    fixture: &ProfilePair,
    registry: &ProgressProjectionRegistry,
    authority: &Authority,
    provider: &dyn ProgressSnapshotProviderPort,
    stop: &CancellationToken,
) -> anyhow::Result<ProgressBound> {
    let sending = fixture
        .outbound
        .request(fixture.peer)
        .connect(Instant::now() + Duration::from_secs(5), stop)
        .await?;
    let register = async {
        let mut stream = fixture
            .receiving
            .open_bi(Instant::now() + Duration::from_secs(5))
            .await?;
        TransferProgressWire
            .register(
                &mut stream,
                &fixture.receiving,
                SOURCE,
                authority,
                &progress_budget(),
                stop,
            )
            .await
            .map_err(anyhow::Error::from)
    };
    let accept = async {
        TransferProgressInbound::serve(
            sending.accept_bi().await?,
            &sending,
            1,
            progress_budget(),
            stop,
            registry,
            provider,
        )
        .await
        .map_err(anyhow::Error::from)
    };
    let (bound, ()) = tokio::try_join!(register, accept)?;
    fixture.outbound.retain(&sending).await?;
    Ok(bound)
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "由run-progress-registration-proof提供当前Application fixture二进制"]
async fn actual_failed_receive_results_cross_sixteen_real_inbound_query_pages() -> anyhow::Result<()>
{
    timeout(Duration::from_secs(30), async {
        let authority=AuthorityProcess::start()?;
        let task=authority.accept(false,1024)?;
        for registered in [true,false] {
            let fixture=ProfilePair::new().await?;
            let clock=Arc::new(SystemElapsedClock);
            let (registry,projection_worker)=ProgressProjectionRegistry::new(clock.clone());
            let key=ProgressProjectionKey { peer:fixture.peer,source:SOURCE };
            registry.accept_intent(key,task.items.to_vec())?;
            let provider=SettledSnapshot(authority.authority.clone());
            let stop=CancellationToken::new();
            if registered {
                let bound=register_current(&fixture,&registry,authority.authority.as_ref(),&provider,&stop).await?;
                ensure!(bound.descriptor==task.descriptor,"real registration must use current receive authority");
            }
            let initial=registry.snapshot(key).unwrap();
            ensure!(initial.items.iter().all(|(_,value)|value.is_none()),"intent and registration cannot invent results");
            ensure!(initial.bound.is_some()==registered,"unregistered fixture must not manufacture a binding");
            let producer=Arc::new(TransferProgressProducer::new(fixture.outbound.clone(), Arc::new(TransferProgressWire), authority.authority.clone(), Arc::new(SettledSnapshot(authority.authority.clone())), clock.clone()));
            let reader=Arc::new(TransferProgressSnapshotReader::new(fixture.outbound.clone(),Arc::new(TransferProgressWire)));
            let (service,exchange_worker)=ProgressExchangeService::new(clock,authority.authority.clone(),producer.clone(),reader,producer);
            let reconciler=TransferProgressReconciler::new(registry.clone(),service);
            let exchange=tokio::spawn(exchange_worker.run(stop.clone()));
            let projection=tokio::spawn(projection_worker.run(stop.clone()));
            let serving=async {
                for _ in 0..16 {
                    TransferProgressInbound::serve(fixture.receiving.accept_bi().await?,&fixture.receiving,1,progress_budget(),&stop,&registry,&provider).await?;
                }
                anyhow::Ok(())
            };
            let reading=async {
                let result=reconciler.refresh(key,&stop).await;
                ensure!(result.is_ok(),"actual reconciliation failed: {:?}",result.as_ref().as_ref().err());
                Ok::<_,anyhow::Error>(result)
            };
            let (read,())=tokio::try_join!(reading,serving)?;
            let Ok(ProgressSnapshotRead::Complete(snapshot))=read.as_ref() else { anyhow::bail!("actual complete query required"); };
            ensure!(snapshot.items.len()==1024,"every original-plan item must be returned");
            ensure!(snapshot.items.iter().all(|item|matches!(item,ProgressItemState::Terminal(value) if matches!(value.outcome,ProgressTerminalOutcome::Failed(_)))),"failed materialization cannot become completed");
            let adopted=registry.snapshot(key).unwrap();
            ensure!(adopted.summary.counts.failed==1024,"all actual failed results must be recovered");
            ensure!(adopted.freshness.unwrap()==ProgressFreshnessView::Settled,"verified terminals must end unknown waiting");
            ensure!(adopted.task==Some(task.descriptor),"first query must learn the actual current scope");
            ensure!(adopted.bound.is_some()==registered,"query must not grant a notification binding");
            if !registered {
                register_current(&fixture,&registry,authority.authority.as_ref(),&provider,&stop).await?;
                ensure!(registry.snapshot(key).unwrap().items==adopted.items,"later matching registration must preserve queried facts");
            }
            registry.close();stop.cancel();exchange.await?;projection.await?;fixture.close().await?;
        }
        anyhow::Ok(())
    }).await??;
    Ok(())
}
