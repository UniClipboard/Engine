use std::sync::Arc;

use thiserror::Error;

use super::{CurrentSpaceMemberScopeError, CurrentSpaceMemberScopePort};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MembershipReadiness {
    Ready,
    Locked,
    Recovering,
}

#[derive(Debug, Error)]
pub enum QueryMembershipReadinessError {
    #[error("membership readiness is unavailable")]
    Unavailable {
        #[source]
        source: CurrentSpaceMemberScopeError,
    },
}

pub(crate) struct QueryMembershipReadinessUseCase {
    current_scope: Arc<dyn CurrentSpaceMemberScopePort>,
}

impl QueryMembershipReadinessUseCase {
    pub(crate) fn new(current_scope: Arc<dyn CurrentSpaceMemberScopePort>) -> Self {
        Self { current_scope }
    }

    pub(crate) async fn execute(
        &self,
    ) -> Result<MembershipReadiness, QueryMembershipReadinessError> {
        match self.current_scope.snapshot().await {
            Ok(scope) if scope.local_member_active => Ok(MembershipReadiness::Ready),
            Ok(_) => Ok(MembershipReadiness::Recovering),
            Err(CurrentSpaceMemberScopeError::Locked) => Ok(MembershipReadiness::Locked),
            Err(
                CurrentSpaceMemberScopeError::NoCurrentSpace
                | CurrentSpaceMemberScopeError::RecoveryRequired,
            ) => Ok(MembershipReadiness::Recovering),
            Err(source @ CurrentSpaceMemberScopeError::Unavailable) => {
                Err(QueryMembershipReadinessError::Unavailable { source })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::space::membership::CurrentSpaceMemberScope;

    struct FixedScope(Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError>);

    #[async_trait]
    impl CurrentSpaceMemberScopePort for FixedScope {
        async fn snapshot(&self) -> Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError> {
            self.0.clone()
        }
    }

    fn query(
        result: Result<CurrentSpaceMemberScope, CurrentSpaceMemberScopeError>,
    ) -> QueryMembershipReadinessUseCase {
        QueryMembershipReadinessUseCase::new(Arc::new(FixedScope(result)))
    }

    #[tokio::test]
    async fn verified_local_membership_is_ready_without_online_peers() {
        let readiness = query(Ok(CurrentSpaceMemberScope {
            revision: 7,
            local_member_active: true,
            usable_peer_device_ids: Vec::new(),
            paused_peer_devices: Vec::new(),
        }))
        .execute()
        .await
        .unwrap();

        assert_eq!(readiness, MembershipReadiness::Ready);
    }

    #[tokio::test]
    async fn missing_authoritative_membership_requires_recovery() {
        let readiness = query(Err(CurrentSpaceMemberScopeError::NoCurrentSpace))
            .execute()
            .await
            .unwrap();

        assert_eq!(readiness, MembershipReadiness::Recovering);
    }

    #[tokio::test]
    async fn inactive_local_membership_requires_recovery() {
        let readiness = query(Ok(CurrentSpaceMemberScope {
            revision: 8,
            local_member_active: false,
            usable_peer_device_ids: Vec::new(),
            paused_peer_devices: Vec::new(),
        }))
        .execute()
        .await
        .unwrap();

        assert_eq!(readiness, MembershipReadiness::Recovering);
    }

    #[tokio::test]
    async fn locked_space_remains_distinct_from_membership_recovery() {
        let readiness = query(Err(CurrentSpaceMemberScopeError::Locked))
            .execute()
            .await
            .unwrap();

        assert_eq!(readiness, MembershipReadiness::Locked);
    }

    #[tokio::test]
    async fn unavailable_scope_preserves_the_source_error() {
        let error = query(Err(CurrentSpaceMemberScopeError::Unavailable))
            .execute()
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            QueryMembershipReadinessError::Unavailable {
                source: CurrentSpaceMemberScopeError::Unavailable
            }
        ));
    }
}
