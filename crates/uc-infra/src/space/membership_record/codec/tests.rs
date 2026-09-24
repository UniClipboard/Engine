use std::collections::BTreeMap;

use uc_core::ids::DeviceId;
use uc_core::membership::{
    MemberEffectKind, MemberEffectMaterial, MembershipDecisionV2, MembershipEventV2,
};

use super::common::{BranchRecoveryDto, InboundTransferDto};
use super::legacy::{self, LegacyDelivery, LegacyInitiatedRemoval, FORMAT_V4};
use super::migrate::effect_material;

const GENERATION: [u8; 16] = [0x49; 16];

/// 从 S0 固定向量取出真实的已签名移除事件与决定。
fn signed_samples() -> (MembershipEventV2, MembershipDecisionV2) {
    let peer = DeviceId::new("device-b");
    let deliveries = |bytes: &[u8]| {
        legacy::decode(FORMAT_V4, bytes)
            .unwrap()
            .1
            .peer_reconciliation
            .remove(&peer)
            .unwrap()
            .restricted_delivery
    };
    let event = match deliveries(include_bytes!(
        "../fixtures/sponsor_removal_notice_pending.v4.bin"
    ))
    .pop()
    {
        Some(LegacyDelivery::Event(event)) => event,
        _ => panic!("fixture keeps an undelivered removal notice"),
    };
    let decision =
        match deliveries(include_bytes!("../fixtures/local_removal_accepted.v4.bin")).pop() {
            Some(LegacyDelivery::Decision(decision)) => decision,
            _ => panic!("fixture keeps an undeliverable decision"),
        };
    (event, decision)
}

/// 由原 Application 类型编码、覆盖全部状态种类的 V4 字节（S3 删除原类型前固定）。
const RICH_V4: &[u8] = include_bytes!("../fixtures/rich_state.v4.bin");

// V4 原由 Application 类型直接序列化。固定向量覆盖全部状态种类，Infra 独立布局必须逐字节读回并
// 写出同样的字节。
#[test]
fn infra_v4_layout_matches_the_application_serialization_byte_for_byte() {
    let (generation, decoded) = legacy::decode(FORMAT_V4, RICH_V4).unwrap();
    assert_eq!(generation, GENERATION);
    assert_eq!(decoded.revision, 12);
    assert_eq!(decoded.effect_journal.len(), 4);
    assert_eq!(decoded.branch_recovery.recovery_sessions.len(), 1);
    assert_eq!(
        postcard::to_stdvec(&(FORMAT_V4, generation, decoded)).unwrap(),
        RICH_V4
    );

    // V5 经上层类型保存同一份分叉与交换资料，转换往返不丢失任何字段。
    let branch_recovery = legacy::decode(FORMAT_V4, RICH_V4)
        .unwrap()
        .1
        .branch_recovery;
    let expected = postcard::to_stdvec(&branch_recovery).unwrap();
    let record = branch_recovery.into_record().unwrap();
    assert_eq!(
        postcard::to_stdvec(&BranchRecoveryDto::from_record(&record)).unwrap(),
        expected
    );
    let transfers = legacy::decode(FORMAT_V4, RICH_V4)
        .unwrap()
        .1
        .inbound_transfers;
    let expected = postcard::to_stdvec(&transfers).unwrap();
    let round_trip: BTreeMap<_, _> = transfers
        .into_iter()
        .map(|(device, transfer)| {
            let transfer = transfer.into_transfer();
            (device, InboundTransferDto::from_transfer(&transfer))
        })
        .collect();
    assert_eq!(postcard::to_stdvec(&round_trip).unwrap(), expected);
}

#[test]
fn effect_payloads_are_classified_by_their_exact_layout() {
    let (event, decision) = signed_samples();
    let initiated = postcard::to_stdvec(&LegacyInitiatedRemoval {
        event: event.clone(),
        retained_device_ids: vec![DeviceId::new("device-c")],
    })
    .unwrap();
    let event_bytes = postcard::to_stdvec(&event).unwrap();
    let decision_bytes = postcard::to_stdvec(&decision).unwrap();

    assert_eq!(
        effect_material(MemberEffectKind::AddDevice, &event_bytes).unwrap(),
        MemberEffectMaterial::Event(event.clone())
    );
    assert_eq!(
        effect_material(MemberEffectKind::RemoveDevice, &event_bytes).unwrap(),
        MemberEffectMaterial::Event(event.clone())
    );
    assert_eq!(
        effect_material(MemberEffectKind::RemoveDevice, &initiated).unwrap(),
        MemberEffectMaterial::InitiatedRemoval {
            event,
            retained_device_ids: vec![DeviceId::new("device-c")],
        }
    );
    assert_eq!(
        effect_material(MemberEffectKind::RemoveDevice, &decision_bytes).unwrap(),
        MemberEffectMaterial::Decision(decision)
    );
    assert!(effect_material(MemberEffectKind::AddDevice, &decision_bytes).is_err());
    assert!(effect_material(MemberEffectKind::RemoveDevice, &[]).is_err());
}
