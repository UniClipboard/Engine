//! 复用同一/proc采样，在真实生命周期验收中检查原进程资源回到基线。
use super::{current_num_threads, Resources};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::time::sleep;

static REPORT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub(in super::super) struct ReceiveResourceBaseline {
    before_shared_cpu_pool: Resources,
    initial: Resources,
    closed_before_case_drop: Option<Resources>,
    directory: PathBuf,
    case: String,
}

impl ReceiveResourceBaseline {
    pub(in super::super) fn capture() -> Option<Self> {
        let directory = std::env::var_os("UC_RECEIVE_RESOURCE_EVIDENCE_DIR")?;
        let before_shared_cpu_pool = Resources::read().expect("读取原生资源冷基线");
        // 进程共享池有意常驻；同时保留冷计数，不把SDK私有线程预先算入基线。
        current_num_threads();
        Some(Self {
            before_shared_cpu_pool,
            initial: Resources::read().expect("读取原生资源基线"),
            closed_before_case_drop: None,
            directory: PathBuf::from(directory),
            case: std::thread::current()
                .name()
                .unwrap_or("unknown_test")
                .to_owned(),
        })
    }

    pub(in super::super) fn record_before_case_drop(&mut self) {
        self.closed_before_case_drop = Some(Resources::read().expect("读取SDK关闭后的资源"));
    }

    pub(in super::super) async fn verify_after_shutdown(self) {
        let closed = Resources::read().expect("读取关闭后的原生资源");
        let started = Instant::now();
        let mut after_idle;
        let settled;
        loop {
            after_idle = Resources::read().expect("读取原生资源空闲收尾");
            let at_baseline = after_idle.fds <= self.initial.fds
                && after_idle.threads <= self.initial.threads
                && after_idle.runtime_tasks <= self.initial.runtime_tasks;
            if at_baseline || started.elapsed() >= Duration::from_secs(45) {
                settled = at_baseline;
                break;
            }
            sleep(Duration::from_millis(100)).await;
        }
        let report = serde_json::json!({
            "case": self.case,
            "before_shared_cpu_pool": self.before_shared_cpu_pool,
            "initial": self.initial,
            "closed_before_case_drop": self.closed_before_case_drop.expect("必须已完成原Engine关闭"),
            "closed": closed,
            "closed_after_idle": after_idle,
            "idle_settlement_ms": started.elapsed().as_millis(),
            "fd_return_to_initial_verified": after_idle.fds <= self.initial.fds,
            "thread_return_to_initial_verified": after_idle.threads <= self.initial.threads,
            "runtime_tasks_return_to_initial_verified": after_idle.runtime_tasks <= self.initial.runtime_tasks,
            "no_sampler_task": true,
            "resource_settlement_verified": settled,
            "tag_counts_observed": false,
            "quota_counts_observed": false,
            "native_blob_connection_counts_observed": false,
        });
        std::fs::create_dir_all(&self.directory).expect("创建资源证据目录");
        let sequence = REPORT_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = self.directory.join(format!(
            "receive-lifecycle-{}-{sequence}.json",
            std::process::id()
        ));
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&report).expect("编码资源计数"),
        )
        .expect("保存资源计数");
        assert!(
            settled,
            "实际生命周期结束后FD、线程与任务必须回到基线；原始计数已保留"
        );
    }
}
