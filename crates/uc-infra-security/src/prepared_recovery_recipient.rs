//! 本机分支恢复收件人暂存：V2 保留真实生成的 commit；旧 V1 不补造应用证据。
use serde::{Deserialize, Serialize};
use uc_core::crypto::EncryptionError;

#[derive(Serialize, Deserialize)]
struct RecipientV1 {
    version: u8,
    mls_state: Vec<u8>,
    wrapping_key: Vec<u8>,
    epoch: u64,
}
#[derive(Serialize, Deserialize)]
struct RecipientV2 {
    version: u8,
    mls_state: Vec<u8>,
    wrapping_key: Vec<u8>,
    epoch: u64,
    external_commit: Vec<u8>,
}
pub(super) struct PreparedRecoveryRecipient {
    pub(super) mls_state: Vec<u8>,
    pub(super) wrapping_key: Vec<u8>,
    pub(super) epoch: u64,
    pub(super) external_commit: Option<Vec<u8>>,
}
pub(super) fn encode(
    mls_state: Vec<u8>,
    wrapping_key: Vec<u8>,
    epoch: u64,
    external_commit: Vec<u8>,
) -> Result<Vec<u8>, anyhow::Error> {
    Ok(postcard::to_stdvec(&RecipientV2 {
        version: 2,
        mls_state,
        wrapping_key,
        epoch,
        external_commit,
    })?)
}
pub(super) fn decode(bytes: &[u8]) -> Result<PreparedRecoveryRecipient, EncryptionError> {
    let (version, _) = postcard::take_from_bytes::<u8>(bytes)
        .map_err(EncryptionError::key_material_corrupt_from)?;
    let (value, trailing) = match version {
        1 => {
            let (value, trailing) = postcard::take_from_bytes::<RecipientV1>(bytes)
                .map_err(EncryptionError::key_material_corrupt_from)?;
            (
                PreparedRecoveryRecipient {
                    mls_state: value.mls_state,
                    wrapping_key: value.wrapping_key,
                    epoch: value.epoch,
                    external_commit: None,
                },
                trailing,
            )
        }
        2 => {
            let (value, trailing) = postcard::take_from_bytes::<RecipientV2>(bytes)
                .map_err(EncryptionError::key_material_corrupt_from)?;
            if value.external_commit.is_empty() {
                return Err(EncryptionError::key_material_corrupt());
            }
            (
                PreparedRecoveryRecipient {
                    mls_state: value.mls_state,
                    wrapping_key: value.wrapping_key,
                    epoch: value.epoch,
                    external_commit: Some(value.external_commit),
                },
                trailing,
            )
        }
        _ => return Err(EncryptionError::key_material_corrupt()),
    };
    if !trailing.is_empty() || value.epoch == 0 {
        return Err(EncryptionError::key_material_corrupt());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_recipient_has_no_commit_evidence_and_final_v2_retains_it() {
        let old = postcard::to_stdvec(&RecipientV1 {
            version: 1,
            mls_state: vec![1],
            wrapping_key: vec![2],
            epoch: 7,
        })
        .unwrap();
        assert!(decode(&old).unwrap().external_commit.is_none());
        let current = encode(vec![1], vec![2], 7, vec![3]).unwrap();
        assert_eq!(decode(&current).unwrap().external_commit, Some(vec![3]));
        let mut trailing = current;
        trailing.push(0xff);
        assert!(decode(&trailing).is_err());
    }
}
