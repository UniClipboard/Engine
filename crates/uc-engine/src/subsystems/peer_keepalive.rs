use std::sync::Arc;

use uc_application::facade::{AppFacade, AppPresenceEvent, AppPresenceSubscriptionError};
use uc_core::TaskRegistry;

use crate::engine::event_stream::EventSender;
use crate::{EngineEvent, PeerPresenceChanged};

pub(crate) async fn spawn_peer_presence_event_task(
    facade: Arc<AppFacade>,
    tasks: &Arc<TaskRegistry>,
    events: EventSender,
) {
    let Ok(mut presence) = facade.subscribe_peer_presence_events() else {
        return;
    };
    tasks
        .spawn("peer_presence_events", move |cancel| async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    event = presence.recv() => match event {
                        Ok(event) => events.send(engine_event_for_presence(&event)),
                        Err(AppPresenceSubscriptionError::Lagged(_)) => {
                            events.send(EngineEvent::RefreshRequired {
                                reason: crate::RefreshReason::ConsumerLagged,
                            });
                        }
                        Err(AppPresenceSubscriptionError::Closed) => return,
                    }
                }
            }
        })
        .await;
}

fn engine_event_for_presence(event: &AppPresenceEvent) -> EngineEvent {
    EngineEvent::PeerPresenceChanged(PeerPresenceChanged {
        device_id: event.device_id.clone(),
        state: event.state.clone(),
        at_ms: event.at_ms,
    })
}
