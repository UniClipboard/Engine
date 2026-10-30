//! 受控协调器场景；不代表真实线程创建失败或资源关闭。
#![allow(dead_code)]

mod engine_activity {
    use std::sync::Arc;

    #[derive(Clone)]
    pub(crate) struct RetainedEngineError {
        pub(crate) source: Arc<dyn std::error::Error + Send + Sync>,
    }

    impl RetainedEngineError {
        pub(crate) fn same_owner(&self, other: &Self) -> bool {
            Arc::ptr_eq(&self.source, &other.source)
        }
    }

    pub(crate) fn fence_engines() -> Arc<()> {
        Arc::new(())
    }
}

mod local_file {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[path = "../src/process_release.rs"]
mod process_release;

use std::time::Duration;

use process_release::{
    ProcessReleaseCoordinator, ProcessReleaseErrors, ProcessReleaseResult, ProcessResumeResult,
    ReleaseStart, ResumeStart,
};

fn invalidation_scenarios() -> bool {
    let active = ProcessReleaseCoordinator::default();
    active.invalidate_for_engine_retention(false);
    if !matches!(
        active.begin_release(),
        ReleaseStart::Started { generation: 1, .. }
    ) {
        return false;
    }

    let released = ProcessReleaseCoordinator::default();
    let (old, old_receipt) = match released.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    released.finish_release(old, true, false, false, ProcessReleaseErrors::default());
    released.invalidate_for_engine_retention(false);
    let current = released.wait_release(Duration::ZERO);
    released.invalidate_for_engine_retention(false);
    if !matches!(old_receipt.wait(Duration::ZERO), ProcessReleaseResult::Released { generation, .. } if generation == old)
        || !matches!(current, ProcessReleaseResult::Failed { generation } if generation > old)
        || released.wait_release(Duration::ZERO) != current
    {
        return false;
    }

    let releasing = ProcessReleaseCoordinator::default();
    let (old, receipt) = match releasing.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    releasing.invalidate_for_engine_retention(false);
    let pending_after_retention = matches!(receipt.wait(Duration::ZERO), ProcessReleaseResult::Pending { generation } if generation == old);
    releasing.finish_release(old, false, false, false, ProcessReleaseErrors::default());
    releasing.finish_release(old, true, false, false, ProcessReleaseErrors::default());
    if !pending_after_retention {
        return false;
    }
    if !matches!(receipt.wait(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == old)
        || !matches!(releasing.wait_release(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == old)
    {
        return false;
    }

    let resume_queued = ProcessReleaseCoordinator::default();
    let (first_generation, first_receipt) = match resume_queued.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    let (resume_generation, resume_receipt) = match resume_queued.begin_resume() {
        ResumeStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    resume_queued.invalidate_for_engine_retention(false);
    if !matches!(
        first_receipt.wait(Duration::ZERO),
        ProcessReleaseResult::Pending { .. }
    ) || !matches!(
        resume_receipt.wait(Duration::ZERO),
        ProcessResumeResult::Pending { .. }
    ) {
        return false;
    }
    resume_queued.finish_release(
        first_generation,
        false,
        false,
        false,
        ProcessReleaseErrors::default(),
    );
    if !matches!(first_receipt.wait(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == first_generation)
        || !matches!(resume_receipt.wait(Duration::ZERO), ProcessResumeResult::Failed { generation } if generation == resume_generation)
        || !matches!(resume_queued.wait_release(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == resume_generation)
    {
        return false;
    }

    let queued = ProcessReleaseCoordinator::default();
    let (old, first) = match queued.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    let (resume_generation, resume) = match queued.begin_resume() {
        ResumeStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    let (last_generation, last) = match queued.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    queued.invalidate_for_engine_retention(false);
    if !matches!(
        first.wait(Duration::ZERO),
        ProcessReleaseResult::Pending { .. }
    ) || !matches!(
        resume.wait(Duration::ZERO),
        ProcessResumeResult::Pending { .. }
    ) || !matches!(
        last.wait(Duration::ZERO),
        ProcessReleaseResult::Pending { .. }
    ) {
        return false;
    }
    queued.finish_release(old, false, false, false, ProcessReleaseErrors::default());
    queued.finish_resume(resume_generation, true);
    if !matches!(first.wait(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == old)
        || !matches!(resume.wait(Duration::ZERO), ProcessResumeResult::Failed { generation } if generation == resume_generation)
        || !matches!(last.wait(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == last_generation)
        || !matches!(queued.wait_release(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == last_generation)
    {
        return false;
    }

    let resuming = ProcessReleaseCoordinator::default();
    let first = match resuming.begin_release() {
        ReleaseStart::Started { generation, .. } => generation,
        _ => return false,
    };
    resuming.finish_release(first, true, false, false, ProcessReleaseErrors::default());
    let (resume_generation, resume) = match resuming.begin_resume() {
        ResumeStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    resuming.invalidate_for_engine_retention(false);
    if !matches!(
        resume.wait(Duration::ZERO),
        ProcessResumeResult::Pending { .. }
    ) {
        return false;
    }
    resuming.finish_resume(resume_generation, false);
    resuming.finish_resume(resume_generation, true);
    if !matches!(resume.wait(Duration::ZERO), ProcessResumeResult::Failed { generation } if generation == resume_generation)
        || !matches!(resuming.wait_release(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == resume_generation)
    {
        return false;
    }

    let release_queued = ProcessReleaseCoordinator::default();
    let first = match release_queued.begin_release() {
        ReleaseStart::Started { generation, .. } => generation,
        _ => return false,
    };
    release_queued.finish_release(first, true, false, false, ProcessReleaseErrors::default());
    let (resume_generation, resume) = match release_queued.begin_resume() {
        ResumeStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    let (last_generation, last) = match release_queued.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => return false,
    };
    release_queued.invalidate_for_engine_retention(false);
    if !matches!(
        resume.wait(Duration::ZERO),
        ProcessResumeResult::Pending { .. }
    ) || !matches!(
        last.wait(Duration::ZERO),
        ProcessReleaseResult::Pending { .. }
    ) {
        return false;
    }
    release_queued.finish_resume(resume_generation, false);
    release_queued.finish_release(
        last_generation,
        false,
        false,
        false,
        ProcessReleaseErrors::default(),
    );
    release_queued.finish_resume(resume_generation, true);
    if !matches!(resume.wait(Duration::ZERO), ProcessResumeResult::Failed { generation } if generation == resume_generation)
        || !matches!(last.wait(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == last_generation)
        || !matches!(release_queued.wait_release(Duration::ZERO), ProcessReleaseResult::Failed { generation } if generation == last_generation)
    {
        return false;
    }

    let no_retry = matches!(
        release_queued.begin_retry_with_engines(false),
        ReleaseStart::Existing { .. }
    );
    let explicit_retry = matches!(
        release_queued.begin_retry_with_engines(true),
        ReleaseStart::Started { .. }
    );
    no_retry && explicit_retry
}

fn main() {
    let invalidation_correct = invalidation_scenarios();
    let owner = ProcessReleaseCoordinator::default();
    let (first, first_receipt) = match owner.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("first release"),
    };
    let (resume, observation) = match owner.begin_resume() {
        ResumeStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("queued resume"),
    };
    let (last, last_receipt) = match owner.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("later release"),
    };
    owner.fail_release_spawn(
        last,
        std::sync::Arc::new(std::io::Error::other("controlled spawn failure")),
    );
    let pending_resume = observation.wait(Duration::ZERO);
    let pending_release = owner.wait_release(Duration::ZERO);
    let first_pending = first_receipt.wait(Duration::ZERO);
    let last_pending = last_receipt.wait(Duration::ZERO);
    owner.finish_release(first, true, true, false, ProcessReleaseErrors::default());
    let ready = owner.wait_resume_ready(resume);
    if ready {
        owner.finish_resume(resume, true);
    }
    let resume_result = observation.wait(Duration::ZERO);
    let release_result = owner.wait_release(Duration::ZERO);
    let first_result = first_receipt.wait(Duration::ZERO);
    let last_result = last_receipt.wait(Duration::ZERO);
    let failed_owner = ProcessReleaseCoordinator::default();
    let (failed_first, failed_first_receipt) = match failed_owner.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("failure case first release"),
    };
    let (failed_resume, failed_observation) = match failed_owner.begin_resume() {
        ResumeStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("failure case resume"),
    };
    let (failed_last, failed_last_receipt) = match failed_owner.begin_release() {
        ReleaseStart::Started {
            generation,
            observation,
            ..
        } => (generation, observation),
        _ => panic!("failure case later release"),
    };
    failed_owner.finish_release(
        failed_first,
        false,
        false,
        false,
        ProcessReleaseErrors::default(),
    );
    let no_reopen = !failed_owner.wait_resume_ready(failed_resume);
    let earlier_failure = failed_observation.wait(Duration::ZERO);
    let terminal_failure = failed_owner.wait_release(Duration::ZERO);
    let first_failed_result = failed_first_receipt.wait(Duration::ZERO);
    let last_failed_result = failed_last_receipt.wait(Duration::ZERO);
    let receipts_correct = matches!(first_pending, ProcessReleaseResult::Pending { generation } if generation == first)
        && matches!(last_pending, ProcessReleaseResult::Pending { generation } if generation == last)
        && matches!(first_result, ProcessReleaseResult::Released { generation, operation_failed: true } if generation == first)
        && matches!(last_result, ProcessReleaseResult::Failed { generation } if generation == last)
        && matches!(first_failed_result, ProcessReleaseResult::Failed { generation } if generation == failed_first)
        && matches!(last_failed_result, ProcessReleaseResult::Failed { generation } if generation == failed_last);
    if let Some(directory) = std::env::var_os("UC_TEST_ARTIFACTS_DIR") {
        std::fs::create_dir_all(&directory).expect("evidence directory");
        std::fs::write(std::path::Path::new(&directory).join("process-spawn-failure.json"), serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"production coordinator with controlled thread creation failure and controlled prior release; no real process resources",
            "engine_retention_invalidation":invalidation_correct,
            "separate_receipts_correct":receipts_correct,
            "resume_pending_before_prior_release":matches!(pending_resume, ProcessResumeResult::Pending { generation } if generation == resume),
            "release_pending_before_prior_release":matches!(pending_release, ProcessReleaseResult::Pending { generation } if generation == last),
            "accepted_resume_remains_ready":ready,
            "resume_superseded":matches!(resume_result, ProcessResumeResult::Superseded { generation } if generation == resume),
            "later_release_failed":matches!(release_result, ProcessReleaseResult::Failed { generation } if generation == last),
            "prior_release_failure_prevents_reopen":no_reopen,
            "prior_release_failure_settles_resume":matches!(earlier_failure, ProcessResumeResult::Failed { generation } if generation == failed_resume),
            "prior_release_failure_preserves_latest_generation":matches!(terminal_failure, ProcessReleaseResult::Failed { generation } if generation == failed_last)
        })).expect("serialize")).expect("write evidence");
    }
    assert!(invalidation_correct && receipts_correct);
    assert!(
        matches!(pending_resume, ProcessResumeResult::Pending { generation } if generation == resume)
    );
    assert!(
        matches!(pending_release, ProcessReleaseResult::Pending { generation } if generation == last)
    );
    assert!(ready, "accepted resume must run after the earlier release");
    assert!(
        matches!(resume_result, ProcessResumeResult::Superseded { generation } if generation == resume)
    );
    assert!(
        matches!(release_result, ProcessReleaseResult::Failed { generation } if generation == last)
    );
    assert!(
        no_reopen
            && matches!(earlier_failure, ProcessResumeResult::Failed { generation } if generation == failed_resume)
    );
    assert!(
        matches!(terminal_failure, ProcessReleaseResult::Failed { generation } if generation == failed_last)
    );
}
