use std::sync::Arc;

use super::durable::DurableAdmissionTransaction;
use crate::space::workspace_membership::{WorkspaceConvergenceError, WorkspaceMembership};

pub(crate) struct SpaceAdmission {
    pub(in crate::space) membership: Arc<WorkspaceMembership>,
    pub(in crate::space) admission: DurableAdmissionTransaction,
}

impl SpaceAdmission {
    pub(crate) fn new(membership: Arc<WorkspaceMembership>) -> Arc<Self> {
        let admission = DurableAdmissionTransaction::new(
            Arc::clone(&membership.deps.admission_attempts),
            Arc::clone(&membership.deps.membership_history_repo),
            Arc::clone(&membership.deps.historical_membership_signatures),
            Arc::clone(&membership.deps.admission_security_transition),
            Arc::clone(&membership.deps.admission_space_transition),
        );
        Arc::new(Self {
            membership,
            admission,
        })
    }
}
