use uc_application::facade::{MembershipReadiness, QueryMembershipReadinessError};

use crate::{
    error_codes::QUERY_MEMBERSHIP_READINESS_UNAVAILABLE_CODE, EngineError, EngineErrorCategory,
    MembershipReadinessStateSummary, MembershipReadinessSummary, OperationResult,
};

pub async fn execute_query_membership_readiness(
    facade: &uc_application::facade::AppFacade,
) -> Result<OperationResult, EngineError> {
    let state = facade
        .query_membership_readiness()
        .await
        .map_err(map_error)?;
    let state = match state {
        MembershipReadiness::Ready => MembershipReadinessStateSummary::Ready,
        MembershipReadiness::Locked => MembershipReadinessStateSummary::Locked,
        MembershipReadiness::Recovering => MembershipReadinessStateSummary::Recovering,
    };
    Ok(OperationResult::MembershipReadiness(
        MembershipReadinessSummary { state },
    ))
}

fn map_error(error: QueryMembershipReadinessError) -> EngineError {
    match error {
        QueryMembershipReadinessError::Unavailable { .. } => EngineError::new(
            QUERY_MEMBERSHIP_READINESS_UNAVAILABLE_CODE,
            EngineErrorCategory::Unavailable,
            true,
        ),
    }
}
