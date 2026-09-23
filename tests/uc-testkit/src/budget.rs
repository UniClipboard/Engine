use std::{
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use serde::Serialize;

#[derive(Clone, Copy, Debug)]
pub struct ScenarioBudget {
    wall_clock: Duration,
}

impl ScenarioBudget {
    pub fn new(wall_clock: Duration) -> Self {
        Self { wall_clock }
    }

    pub(crate) fn remaining(self, started_at: Instant) -> Duration {
        self.wall_clock.saturating_sub(started_at.elapsed())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StageTiming {
    pub name: String,
    pub elapsed_ms: u64,
}

pub struct StageGuard {
    name: &'static str,
    started_at: Instant,
    timings: Arc<Mutex<Vec<StageTiming>>>,
}

impl StageGuard {
    pub(crate) fn new(name: &'static str, timings: Arc<Mutex<Vec<StageTiming>>>) -> Self {
        Self {
            name,
            started_at: Instant::now(),
            timings,
        }
    }
}

impl Drop for StageGuard {
    fn drop(&mut self) {
        lock(&self.timings).push(StageTiming {
            name: self.name.to_owned(),
            elapsed_ms: millis(self.started_at.elapsed()),
        });
    }
}

pub(crate) fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
