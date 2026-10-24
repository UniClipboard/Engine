//! 仅测试运行时记录任务出生与真实结束；输出组件、代码位置摘要和计数，不记录任务内容或路径。
// Tokio 的任务回调由仓库 tokio_unstable 构建开关启用；兼容未启用该开关的常规测试构建。
#![allow(unexpected_cfgs)]
#[cfg(tokio_unstable)]
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
#[cfg(tokio_unstable)]
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::runtime::Builder;
use tokio::task::Id;

#[derive(Clone, Default)]
pub(super) struct TaskOrigins {
    alive: Arc<Mutex<HashMap<Id, String>>>,
}

impl TaskOrigins {
    pub fn install(&self, builder: &mut Builder) {
        #[cfg(tokio_unstable)]
        {
            let spawned = self.alive.clone();
            builder.on_task_spawn(move |meta| {
                let location = meta.spawned_at();
                let file = location.file();
                let component = [
                    ("uc-application/src/transfer", "transfer"),
                    ("uc-application/src/connectivity", "connectivity"),
                    ("uc-application/src/space", "space"),
                    ("uc-application/src/clipboard", "clipboard"),
                    ("uc-application/src/search", "search"),
                    ("uc-core/src/task_registry", "supervised_task"),
                    ("uc-engine", "engine"),
                    ("uc-infra-p2p", "p2p_adapter"),
                    ("iroh-relay-", "relay"),
                    ("iroh-", "iroh"),
                    ("quinn-", "quinn"),
                    ("noq-", "quic"),
                    ("acto-", "actor"),
                    ("hyper-util-", "http"),
                    ("n0-future-", "future_runtime"),
                    ("hickory-", "dns"),
                    ("netwatch-", "netwatch"),
                    ("portmapper-", "portmapper"),
                    ("wiremock-", "test_server"),
                    ("tokio-", "tokio"),
                ]
                .into_iter()
                .find_map(|(needle, label)| file.contains(needle).then_some(label))
                .unwrap_or("dependency");
                let mut digest = DefaultHasher::new();
                file.hash(&mut digest);
                let key = format!("{component}_{}_{:016x}", location.line(), digest.finish());
                spawned
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(meta.id(), key);
            });
            let ended = self.alive.clone();
            builder.on_task_terminate(move |meta| {
                ended
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&meta.id());
            });
        }
        #[cfg(not(tokio_unstable))]
        {
            let _ = builder;
            panic!("显式任务来源诊断需要 tokio_unstable 构建开关");
        }
    }

    pub fn snapshot(&self) -> BTreeMap<String, usize> {
        let mut counts = BTreeMap::new();
        for origin in self
            .alive
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            *counts.entry(origin.clone()).or_default() += 1;
        }
        counts
    }
}
