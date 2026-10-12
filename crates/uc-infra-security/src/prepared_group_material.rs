//! 准入/分支恢复的本地暂存材料。V2 有显式标记，旧无标记布局读取时不补造应用证据。
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use uc_core::ids::DeviceId;
use uc_core::membership::{
    AdmissionReplayId, PendingGroupUpdate, ProtectionGroupAdmission, SpaceKeyMaterial,
    SpaceKeyState,
};

// 旧布局首字段是 SpaceId 字符串长度；十个 ff 是非法 postcard u64，不能与合法旧资料混淆。
const V2: &[u8] = b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\xffUCM2";

#[derive(Deserialize, Serialize)]
struct LegacyMaterial {
    state: SpaceKeyState,
    group_state: Vec<u8>,
    key_catalog: Vec<u8>,
    pending_group_updates: Vec<PendingGroupUpdate>,
    pending_group_admission_replays: Vec<LegacyReplay>,
    updated_at_ms: i64,
}
#[derive(Deserialize, Serialize)]
struct LegacyReplay {
    recipient: DeviceId,
    replay_id: AdmissionReplayId,
    admission: ProtectionGroupAdmission,
}

pub(super) fn encode(material: &SpaceKeyMaterial) -> Result<Vec<u8>, anyhow::Error> {
    let mut encoded = V2.to_vec();
    encoded.extend(postcard::to_stdvec(material)?);
    Ok(encoded)
}
pub(super) fn decode(encoded: &[u8]) -> Result<SpaceKeyMaterial, anyhow::Error> {
    if let Some(body) = encoded.strip_prefix(V2) {
        let (material, trailing) = postcard::take_from_bytes::<SpaceKeyMaterial>(body)?;
        anyhow::ensure!(
            trailing.is_empty(),
            "prepared group material has trailing bytes"
        );
        return Ok(material);
    }
    let (legacy, trailing) = postcard::take_from_bytes::<LegacyMaterial>(encoded)?;
    anyhow::ensure!(
        trailing.is_empty(),
        "legacy prepared group material has trailing bytes"
    );
    let mut material = SpaceKeyMaterial::new(
        legacy.state,
        legacy.group_state,
        legacy.key_catalog,
        legacy.updated_at_ms,
    );
    material.add_pending_group_updates(legacy.pending_group_updates, legacy.updated_at_ms);
    let mut recipients = HashSet::new();
    for replay in legacy.pending_group_admission_replays {
        anyhow::ensure!(
            recipients.insert(replay.recipient),
            "legacy admission replay has duplicate recipient"
        );
        material.cache_group_admission(
            replay.recipient,
            replay.replay_id,
            replay.admission,
            legacy.updated_at_ms,
        );
    }
    Ok(material)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uc_core::ids::SpaceId;
    use uc_core::membership::GroupEpoch;

    #[test]
    fn legacy_shape_is_read_without_inventing_evidence_and_v2_preserves_it() {
        let state = SpaceKeyState::legacy(SpaceId::from("fixture"));
        let old = LegacyMaterial {
            state: state.clone(),
            group_state: vec![1],
            key_catalog: vec![2],
            pending_group_updates: vec![],
            pending_group_admission_replays: vec![],
            updated_at_ms: 17,
        };
        let original = postcard::to_stdvec(&old).unwrap();
        let mut current = decode(&original).unwrap();
        assert_eq!(current.updated_at_ms(), 17);
        assert!(current
            .applied_group_update_digest(GroupEpoch::new(0))
            .is_none());
        current
            .record_applied_group_update(GroupEpoch::new(0), [0x41; 32])
            .unwrap();
        let encoded = encode(&current).unwrap();
        assert!(encoded.starts_with(V2));
        assert_eq!(decode(&encoded).unwrap(), current);
        let mut trailing = encoded;
        trailing.push(0xff);
        assert!(decode(&trailing).is_err());
    }
}
