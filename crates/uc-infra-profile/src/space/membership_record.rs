//! 成员记录仓储：成员记录的密文格式、旧格式迁移与条件提交。
//!
//! 记录以 profile 密钥 AEAD 加密后保存在 `membership_ledger_state` 单行中。唯一最终格式为
//! `MembershipLedgerRecordV5`；读取到 V1–V4 时在同一事务内迁移并写回 V5，迁移失败不改写原资料。

mod codec;
mod store;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

pub use store::SqliteMembershipRecordStore;
