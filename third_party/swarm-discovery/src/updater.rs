// Modified by UniClipboard: a closed updater mailbox terminates delayed GC work.
use crate::{Callback, Peer};
use acto::{AcTokioRuntime, ActoCell, ActoInput, ActoRef};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};
use tokio::time::sleep;

pub enum Input {
    Peers(BTreeMap<String, Peer>),
    GC,
    SizeSubscription(ActoRef<usize>),
}

fn gc(me: ActoRef<Input>, interval: Duration) {
    tokio::spawn(async move {
        sleep(interval).await;
        let _ = me.send(Input::GC);
    });
}

pub async fn updater(
    mut ctx: ActoCell<Input, AcTokioRuntime>,
    tau: Duration,
    phi: f32,
    mut callback: Callback,
) {
    let gc_interval = tau * 12345 / 9999;
    gc(ctx.me(), gc_interval);

    let mut peers = BTreeMap::new();
    #[allow(clippy::mutable_key_type)]
    let mut subscribers = BTreeSet::<ActoRef<usize>>::new();
    while let ActoInput::Message(msg) = ctx.recv().await {
        match msg {
            Input::Peers(msg) => {
                for (id, peer) in msg {
                    callback(&id, &peer);
                    if peers.insert(id, peer).is_none() {
                        for sub in &subscribers {
                            sub.send(peers.len());
                        }
                    }
                }
            }
            Input::GC => {
                gc(ctx.me(), gc_interval);
                if peers.is_empty() {
                    continue;
                }
                let now = Instant::now();
                // we send min(swarmsize, ceil(tau * phi)) per cadence
                let expected_frequency =
                    (tau.as_secs_f32() * phi).ceil().min(peers.len() as f32) / tau.as_secs_f32();
                let frequency_per_peer = expected_frequency / peers.len() as f32;
                // take per-peer cadence times three to account for jitter
                let per_peer_grace_period = Duration::from_secs_f32(3.0 / frequency_per_peer);
                peers.retain(|peer_id, peer| {
                    let age = now
                        .checked_duration_since(peer.last_seen)
                        .unwrap_or_default();
                    let keep = age < per_peer_grace_period;
                    if !keep {
                        callback(
                            peer_id,
                            &Peer {
                                last_seen: peer.last_seen,
                                addrs: vec![],
                                txt: Default::default(),
                            },
                        );
                    }
                    keep
                });
                for sub in &subscribers {
                    sub.send(peers.len());
                }
            }
            Input::SizeSubscription(sub) => {
                subscribers.insert(sub);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use acto::{AcTokio, ActoHandle, ActoRuntime};
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tracing::{Event, Subscriber};
    use tracing_subscriber::{
        layer::{Context, Layer},
        prelude::*,
        registry::LookupSpan,
    };

    #[derive(Clone)]
    struct ClosedMailboxSendCounter(Arc<AtomicUsize>);

    impl<S> Layer<S> for ClosedMailboxSendCounter
    where
        S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    {
        fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
            if event.metadata().target() == "acto::tokio" {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    #[tokio::test]
    async fn closed_mailbox_gc_does_not_retry_after_the_first_send() {
        let system = AcTokio::from_handle("updater-gc-test", tokio::runtime::Handle::current());
        let actor = system.spawn_actor("closed-updater", |_ctx: ActoCell<Input, _>| async {});
        let target = actor.me;
        actor.handle.join().await.expect("actor exits cleanly");
        assert!(target.is_gone());

        let sends = Arc::new(AtomicUsize::new(0));
        let subscriber =
            tracing_subscriber::registry().with(ClosedMailboxSendCounter(sends.clone()));
        let _subscriber_guard = tracing::subscriber::set_default(subscriber);
        gc(target, Duration::from_millis(1));
        sleep(Duration::from_millis(55)).await;

        assert_eq!(sends.load(Ordering::Relaxed), 1);
    }
}
