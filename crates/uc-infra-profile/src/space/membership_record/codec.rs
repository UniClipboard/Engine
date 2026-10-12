//! 成员记录明文编解码：写入只产生 V6；读取 V1–V5 时一次性迁移到最终形状。

use serde::Deserialize;
use uc_application::deps::{MembershipLedgerError, MembershipRecord};
use uc_core::membership::HistoricalMembershipSignatureVerifier;

mod common;
mod legacy;
mod migrate;
#[cfg(test)]
mod tests;
mod v5;
mod v6;
#[cfg(test)]
pub(super) use v5::encode as encode_legacy_v5;

pub(super) enum Decoded {
    Current(MembershipRecord),
    /// 由旧格式迁移而来，调用方须以 V6 写回后才算迁移完成。
    Migrated(MembershipRecord),
}

pub(super) fn decode(
    bytes: &[u8],
    generation: [u8; 16],
    verifier: &dyn HistoricalMembershipSignatureVerifier,
    now_ms: i64,
) -> Result<Decoded, MembershipLedgerError> {
    let (version, _) =
        postcard::take_from_bytes::<u16>(bytes).map_err(MembershipLedgerError::corrupt_from)?;
    if version == v6::FORMAT_V6 {
        return v6::decode(bytes, generation, verifier).map(Decoded::Current);
    }
    if version == v5::FORMAT_V5 {
        return v5::decode(bytes, generation, verifier).map(Decoded::Migrated);
    }
    let (profile_generation, ledger) = legacy::decode(version, bytes)?;
    if profile_generation != generation {
        return Err(MembershipLedgerError::corrupt());
    }
    migrate::migrate(ledger, verifier, now_ms).map(Decoded::Migrated)
}

pub(super) fn encode(
    record: &MembershipRecord,
    generation: [u8; 16],
) -> Result<Vec<u8>, MembershipLedgerError> {
    v6::encode(record, generation)
}

/// 按完整长度解析；有尾随字节视为损坏。
fn parse<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, MembershipLedgerError> {
    let (value, tail) =
        postcard::take_from_bytes(bytes).map_err(MembershipLedgerError::corrupt_from)?;
    if !tail.is_empty() {
        return Err(MembershipLedgerError::corrupt());
    }
    Ok(value)
}
