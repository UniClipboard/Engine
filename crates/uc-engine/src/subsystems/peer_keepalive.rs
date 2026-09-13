use std::sync::Arc;

use uc_application::facade::{
    AppFacade, AppPeerReachabilityEvent, AppPeerReachabilitySubscriptionError,
};
use uc_core::TaskRegistry;

use crate::engine::event_stream::EventSender;
use crate::{EngineEvent, PeerPresenceChanged};

pub(crate) async fn spawn_peer_reachability_event_task(
    facade: Arc<AppFacade>,
    tasks: &Arc<TaskRegistry>,
    events: EventSender,
) {
    let Ok(mut peer_reachability) = facade.subscribe_peer_reachability_events() else {
        return;
    };
    let _ = tasks
        .spawn(move |cancel| async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    event = peer_reachability.recv() => match event {
                        Ok(event) => events.send(engine_event_for_peer_reachability(&event)),
                        Err(AppPeerReachabilitySubscriptionError::Lagged(_)) => {
                            events.send(EngineEvent::RefreshRequired {
                                reason: crate::RefreshReason::ConsumerLagged,
                            });
                        }
                        Err(AppPeerReachabilitySubscriptionError::Closed) => return,
                    }
                }
            }
        })
        .await;
}

fn engine_event_for_peer_reachability(event: &AppPeerReachabilityEvent) -> EngineEvent {
    EngineEvent::PeerPresenceChanged(PeerPresenceChanged {
        device_id: event.device_id.clone(),
        state: event.state.clone(),
        at_ms: event.at_ms,
    })
}
