//! 连续性故障只退休旧预算；各轮租约不得跨故障世代继续发送。
use super::{ElapsedClockError, ElapsedClockPort, ElapsedClockRefusal};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;
use tokio::sync::Notify;

#[derive(Clone, Copy)]
pub(super) struct Sample {
    pub now: Duration,
    pub epoch: u64,
}
struct State {
    previous: Option<Duration>,
    epoch: u64,
    exhausted: bool,
}
pub(super) struct Clock {
    raw: Arc<dyn ElapsedClockPort>,
    state: Mutex<State>,
    wake: Arc<Notify>,
}
impl Clock {
    pub fn new(raw: Arc<dyn ElapsedClockPort>, wake: Arc<Notify>) -> Self {
        Self {
            raw,
            state: Mutex::new(State {
                previous: None,
                epoch: 0,
                exhausted: false,
            }),
            wake,
        }
    }
    pub fn sample(&self) -> Result<Sample, ElapsedClockError> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.exhausted {
            return Err(retired());
        }
        match self.raw.elapsed() {
            Ok(now) if state.previous.is_none_or(|previous| now >= previous) => {
                state.previous = Some(now);
                Ok(Sample {
                    now,
                    epoch: state.epoch,
                })
            }
            result => {
                let lost = state.previous.is_some();
                state.previous = result.as_ref().ok().copied();
                if lost {
                    match state.epoch.checked_add(1) {
                        Some(next) => state.epoch = next,
                        None => state.exhausted = true,
                    }
                    self.wake.notify_one();
                }
                match result {
                    Err(source) => Err(source),
                    Ok(_) => Err(ElapsedClockError {
                        source: Box::new(ElapsedClockRefusal::NonMonotonicReading),
                    }),
                }
            }
        }
    }
    pub fn epoch(&self) -> Option<u64> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (!state.exhausted).then_some(state.epoch)
    }
    pub fn lease(self: &Arc<Self>, epoch: u64) -> Arc<dyn ElapsedClockPort> {
        Arc::new(Lease {
            clock: self.clone(),
            epoch,
        })
    }
}
struct Lease {
    clock: Arc<Clock>,
    epoch: u64,
}
impl ElapsedClockPort for Lease {
    fn elapsed(&self) -> Result<Duration, ElapsedClockError> {
        let sample = self.clock.sample()?;
        if sample.epoch != self.epoch {
            return Err(retired());
        }
        Ok(sample.now)
    }
}
fn retired() -> ElapsedClockError {
    ElapsedClockError {
        source: Box::new(ElapsedClockRefusal::ContinuityLost),
    }
}
