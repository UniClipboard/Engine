//! 一次准入尝试的关联引用：joiner 与 sponsor 都从同一个 `admission_id` 派生，使两端记录能够对上，
//! 又不暴露原始标识。
//!
//! 引用只由单次尝试的 `admission_id` 决定，换一次尝试就变，所以不能跨尝试、也不能跨邀请识别设备或空间；
//! 派生使用带域名的 SHA-256 并截取 8 字节，无法还原 `admission_id`。
use std::fmt;

use serde::de::{Error as _, Unexpected};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

const DOMAIN: &[u8] = b"uniclipboard.admission-attempt-ref.v1";
const REF_BYTES: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AdmissionAttemptRef([u8; REF_BYTES]);

impl AdmissionAttemptRef {
    /// 从一次尝试的 `admission_id` 派生引用。
    pub fn derive(admission_id: &[u8; 32]) -> Self {
        let digest = Sha256::new()
            .chain_update(DOMAIN)
            .chain_update(admission_id)
            .finalize();
        let mut bytes = [0u8; REF_BYTES];
        bytes.copy_from_slice(&digest[..REF_BYTES]);
        Self(bytes)
    }
}

impl fmt::Display for AdmissionAttemptRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
            .iter()
            .try_for_each(|byte| write!(formatter, "{byte:02x}"))
    }
}

impl fmt::Debug for AdmissionAttemptRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "AdmissionAttemptRef({self})")
    }
}

impl Serialize for AdmissionAttemptRef {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AdmissionAttemptRef {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        parse_ref(&text).ok_or_else(|| {
            D::Error::invalid_value(Unexpected::Str("attempt ref"), &"16 lowercase hex digits")
        })
    }
}

fn parse_ref(text: &str) -> Option<AdmissionAttemptRef> {
    let nibble = |byte: u8| match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    };
    if text.len() != REF_BYTES * 2 {
        return None;
    }
    let mut bytes = [0u8; REF_BYTES];
    for (slot, pair) in bytes.iter_mut().zip(text.as_bytes().chunks(2)) {
        *slot = nibble(pair[0])? << 4 | nibble(pair[1])?;
    }
    Some(AdmissionAttemptRef(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_attempt_derives_the_same_ref_and_other_attempts_differ() {
        let first = AdmissionAttemptRef::derive(&[0x11; 32]);
        assert_eq!(first, AdmissionAttemptRef::derive(&[0x11; 32]));
        assert_ne!(first, AdmissionAttemptRef::derive(&[0x12; 32]));
        assert_eq!(first.to_string().len(), 16);
    }

    #[test]
    fn the_ref_does_not_contain_the_admission_id_and_round_trips() {
        let id = [0xab; 32];
        let attempt = AdmissionAttemptRef::derive(&id);
        assert!(!attempt.to_string().contains("abab"));
        let json = serde_json::to_string(&attempt).expect("serialize");
        assert_eq!(
            serde_json::from_str::<AdmissionAttemptRef>(&json).expect("deserialize"),
            attempt
        );
    }

    #[test]
    fn malformed_refs_are_rejected() {
        for bad in [
            "",
            "abc",
            "ABCDEF0123456789",
            "zzzzzzzzzzzzzzzz",
            "0123456789abcdef0",
        ] {
            assert!(
                serde_json::from_value::<AdmissionAttemptRef>(serde_json::json!(bad)).is_err(),
                "{bad}"
            );
        }
    }
}
