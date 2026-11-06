use super::startup::StartupProgressInput;
use super::startup_control::StartupLifecycleInput;
use super::{StartupLifecycle, StartupProgress};

/// 一次启动交入的一次性能力：只读进度观察与启动期暂停/恢复控制。
///
/// 两个输入都只能交给一次启动；忽略观察者的宿主使用 [`StartupSignals::unobserved`]，
/// 只关心其中一项的宿主使用 [`StartupSignals::new`] 组合已持有的输入。
pub struct StartupSignals {
    pub(super) progress: StartupProgressInput,
    pub(super) lifecycle: StartupLifecycleInput,
}

impl StartupSignals {
    /// 不观察启动进度，也不在启动期接受暂停/恢复通知。
    pub fn unobserved() -> Self {
        let (progress, _progress_observer) = StartupProgress::channel();
        let (lifecycle, _lifecycle_control) = StartupLifecycle::channel();
        Self {
            progress,
            lifecycle,
        }
    }

    /// 同时取得本次启动的进度观察者与启动期控制句柄。
    pub fn channel() -> (Self, StartupProgress, StartupLifecycle) {
        let (progress, progress_observer) = StartupProgress::channel();
        let (lifecycle, lifecycle_control) = StartupLifecycle::channel();
        (
            Self {
                progress,
                lifecycle,
            },
            progress_observer,
            lifecycle_control,
        )
    }

    /// 组合宿主已分别持有的输入，例如先创建控制句柄、后创建进度观察者的绑定层。
    pub fn new(progress: StartupProgressInput, lifecycle: StartupLifecycleInput) -> Self {
        Self {
            progress,
            lifecycle,
        }
    }

    /// 只观察进度，不在启动期接受暂停/恢复通知。
    pub fn with_progress(progress: StartupProgressInput) -> Self {
        let (lifecycle, _lifecycle_control) = StartupLifecycle::channel();
        Self {
            progress,
            lifecycle,
        }
    }
}
