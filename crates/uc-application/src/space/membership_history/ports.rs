use async_trait::async_trait;

use super::MembershipHistoryRepositoryError;

/// 当前 Space 签名成员历史的持久化接口。
///
/// 普通读取和替换独立于准入记录；只有准入记录与成员历史必须共同提交时，
/// 才使用准入仓储提供的联合提交能力。
#[async_trait]
pub trait MembershipHistoryRepositoryPort: Send + Sync {
    async fn load_membership_history(
        &self,
    ) -> Result<Option<Vec<u8>>, MembershipHistoryRepositoryError>;

    async fn compare_and_replace_membership_history(
        &self,
        expected_membership_history_v2: Option<&[u8]>,
        membership_history_v2: &[u8],
    ) -> Result<u64, MembershipHistoryRepositoryError>;
}
