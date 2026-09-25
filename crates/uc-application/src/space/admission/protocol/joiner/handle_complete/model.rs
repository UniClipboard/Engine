use uc_core::membership::{AdmissionSpaceTransition, AdmissionStagedTarget};

pub struct PreparedJoinerActivation {
    transition: AdmissionSpaceTransition,
    staged_target: AdmissionStagedTarget,
}

impl PreparedJoinerActivation {
    /// `staged_target` 替换准入记录中的暂存目标，补入激活执行所需、此前尚未保存的资料。
    pub fn new(transition: AdmissionSpaceTransition, staged_target: AdmissionStagedTarget) -> Self {
        Self {
            transition,
            staged_target,
        }
    }

    pub(crate) fn into_parts(self) -> (AdmissionSpaceTransition, AdmissionStagedTarget) {
        (self.transition, self.staged_target)
    }
}
