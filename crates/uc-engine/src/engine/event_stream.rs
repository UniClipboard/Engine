use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tokio::sync::{broadcast, Notify};

use crate::{EngineEvent, RefreshReason};

pub struct EventStream {
    receiver: broadcast::Receiver<EngineEvent>,
    refresh: Arc<RefreshGate>,
    lag_pending: bool,
}

impl EventStream {
    pub async fn next(&mut self) -> Option<EngineEvent> {
        loop {
            let released = self.refresh.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            if self.lag_pending && !self.refresh.is_held() {
                self.lag_pending = false;
                return Some(lagged());
            }
            tokio::select! {
                biased;
                _ = released, if self.lag_pending => {}
                received = self.receiver.recv() => match received {
                    Ok(event) => return Some(event),
                    // 积压同样要求宿主重新查询：暂存期间先记下，照常交出其他事件，释放后再补交。
                    Err(broadcast::error::RecvError::Lagged(_)) if self.refresh.is_held() => {
                        self.lag_pending = true;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => return Some(lagged()),
                    Err(broadcast::error::RecvError::Closed) => return None,
                },
            }
        }
    }
}

fn lagged() -> EngineEvent {
    EngineEvent::RefreshRequired {
        reason: RefreshReason::ConsumerLagged,
    }
}

/// 接收端只需知道暂存是否进行中；不持有发送端，最后一个 `EventSender` 丢弃时通道照常关闭。
#[derive(Default)]
struct RefreshGate {
    held: AtomicBool,
    released: Notify,
}

impl RefreshGate {
    fn is_held(&self) -> bool {
        self.held.load(Ordering::SeqCst)
    }
}

#[derive(Clone)]
pub(crate) struct EventSender {
    state: Arc<Mutex<EventSenderState>>,
    refresh: Arc<RefreshGate>,
}

struct EventSenderState {
    sender: Option<broadcast::Sender<EngineEvent>>,
    refresh_holds: usize,
    held: HeldRefresh,
}

/// 暂存期间合并的重新查询通知：只保留最大设备组修订号和各类刷新原因各一次。
#[derive(Default)]
struct HeldRefresh {
    device_trust_revision: Option<u64>,
    refresh_reasons: Vec<RefreshReason>,
}

impl EventSender {
    pub(crate) fn send(&self, event: EngineEvent) {
        let mut state = self.lock_state();
        if state.refresh_holds > 0 {
            match event {
                EngineEvent::DeviceTrustChanged { revision } => {
                    let held = &mut state.held.device_trust_revision;
                    *held = Some(held.map_or(revision, |current| current.max(revision)));
                    return;
                }
                EngineEvent::RefreshRequired { reason } => {
                    if !state.held.refresh_reasons.contains(&reason) {
                        state.held.refresh_reasons.push(reason);
                    }
                    return;
                }
                _ => {}
            }
        }
        if let Some(sender) = state.sender.as_ref() {
            let _ = sender.send(event);
        }
    }

    /// 暂存要求宿主重新查询的事件，直到返回的守卫全部释放后按原类别补发。
    ///
    /// 会话切换期间当前会话已拆除，宿主此时重新查询只会得到不可用错误；
    /// 切换负责人在新会话可读之前持有守卫，保证通知送达时查询能读到对应状态。
    pub(crate) fn hold_refresh(&self) -> RefreshHold {
        let mut state = self.lock_state();
        state.refresh_holds += 1;
        self.refresh.held.store(true, Ordering::SeqCst);
        drop(state);
        RefreshHold {
            events: self.clone(),
        }
    }

    pub(crate) fn close(&self) {
        {
            let mut state = self.lock_state();
            state.sender.take();
            state.held = HeldRefresh::default();
            self.refresh.held.store(false, Ordering::SeqCst);
        }
        self.refresh.released.notify_waiters();
    }

    fn release_refresh_hold(&self) {
        {
            let mut state = self.lock_state();
            state.refresh_holds = state.refresh_holds.saturating_sub(1);
            if state.refresh_holds > 0 {
                return;
            }
            let held = std::mem::take(&mut state.held);
            if let Some(sender) = state.sender.as_ref() {
                if let Some(revision) = held.device_trust_revision {
                    let _ = sender.send(EngineEvent::DeviceTrustChanged { revision });
                }
                for reason in held.refresh_reasons {
                    let _ = sender.send(EngineEvent::RefreshRequired { reason });
                }
            }
            self.refresh.held.store(false, Ordering::SeqCst);
        }
        self.refresh.released.notify_waiters();
    }

    fn lock_state(&self) -> MutexGuard<'_, EventSenderState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// 刷新事件暂存守卫；释放最后一个守卫时补发暂存期间合并的通知。
pub(crate) struct RefreshHold {
    events: EventSender,
}

impl Drop for RefreshHold {
    fn drop(&mut self) {
        self.events.release_refresh_hold();
    }
}

pub(crate) fn event_channel(capacity: usize) -> (EventSender, EventStream) {
    let (sender, receiver) = broadcast::channel(capacity);
    let refresh = Arc::new(RefreshGate::default());
    (
        EventSender {
            state: Arc::new(Mutex::new(EventSenderState {
                sender: Some(sender),
                refresh_holds: 0,
                held: HeldRefresh::default(),
            })),
            refresh: Arc::clone(&refresh),
        },
        EventStream {
            receiver,
            refresh,
            lag_pending: false,
        },
    )
}

#[cfg(test)]
mod tests {
    use crate::{EngineEvent, EngineState, RefreshReason};

    use super::event_channel;

    #[tokio::test]
    async fn lagged_consumer_receives_refresh_before_continuing() {
        let (events, mut stream) = event_channel(2);

        events.send(EngineEvent::StateChanged {
            state: EngineState::Quiescing,
        });
        events.send(EngineEvent::StateChanged {
            state: EngineState::Quiesced,
        });
        events.send(EngineEvent::StateChanged {
            state: EngineState::Suspended,
        });

        assert_eq!(
            stream.next().await,
            Some(EngineEvent::RefreshRequired {
                reason: RefreshReason::ConsumerLagged,
            })
        );
        assert_eq!(
            stream.next().await,
            Some(EngineEvent::StateChanged {
                state: EngineState::Quiesced,
            })
        );
    }

    #[tokio::test]
    async fn refresh_hold_coalesces_requery_events_until_every_guard_is_released() {
        let (events, mut stream) = event_channel(8);

        let outer = events.hold_refresh();
        let inner = events.hold_refresh();
        events.send(EngineEvent::DeviceTrustChanged { revision: 3 });
        events.send(EngineEvent::RefreshRequired {
            reason: RefreshReason::StateInvalidated,
        });
        events.send(EngineEvent::DeviceTrustChanged { revision: 2 });
        events.send(EngineEvent::RefreshRequired {
            reason: RefreshReason::StateInvalidated,
        });
        events.send(EngineEvent::StateChanged {
            state: EngineState::Quiesced,
        });
        assert_eq!(
            stream.next().await,
            Some(EngineEvent::StateChanged {
                state: EngineState::Quiesced,
            })
        );

        drop(inner);
        events.send(EngineEvent::StateChanged {
            state: EngineState::Suspended,
        });
        assert_eq!(
            stream.next().await,
            Some(EngineEvent::StateChanged {
                state: EngineState::Suspended,
            })
        );

        drop(outer);
        assert_eq!(
            stream.next().await,
            Some(EngineEvent::DeviceTrustChanged { revision: 3 })
        );
        assert_eq!(
            stream.next().await,
            Some(EngineEvent::RefreshRequired {
                reason: RefreshReason::StateInvalidated,
            })
        );

        events.send(EngineEvent::DeviceTrustChanged { revision: 4 });
        assert_eq!(
            stream.next().await,
            Some(EngineEvent::DeviceTrustChanged { revision: 4 })
        );
    }

    #[tokio::test]
    async fn consumer_lag_during_a_refresh_hold_is_reported_after_release() {
        let (events, mut stream) = event_channel(2);
        let hold = events.hold_refresh();
        for state in [
            EngineState::Quiescing,
            EngineState::Quiesced,
            EngineState::Suspended,
        ] {
            events.send(EngineEvent::StateChanged { state });
        }

        // 暂存期间其他事件照常交出，积压通知不提前出现。
        for state in [EngineState::Quiesced, EngineState::Suspended] {
            assert_eq!(
                stream.next().await,
                Some(EngineEvent::StateChanged { state })
            );
        }
        let lagged = tokio::spawn(async move {
            let next = stream.next().await;
            (next, stream)
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!lagged.is_finished());

        drop(hold);
        let (next, _stream) = lagged.await.expect("lagged consumer task");
        assert_eq!(
            next,
            Some(EngineEvent::RefreshRequired {
                reason: RefreshReason::ConsumerLagged,
            })
        );
    }

    #[tokio::test]
    async fn dropping_the_last_sender_ends_the_stream() {
        let (events, mut stream) = event_channel(2);
        events.send(EngineEvent::StateChanged {
            state: EngineState::Quiesced,
        });
        drop(events);

        assert_eq!(
            stream.next().await,
            Some(EngineEvent::StateChanged {
                state: EngineState::Quiesced,
            })
        );
        assert_eq!(stream.next().await, None);
    }
}
