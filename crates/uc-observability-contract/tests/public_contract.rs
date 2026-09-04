use uc_observability_contract::analytics::{AnalyticsPort, Event, NoopAnalyticsSink};

#[test]
fn event_names_remain_stable_without_a_runtime_sink() {
    let event = Event::AppOpened;
    assert_eq!(event.name(), "app_opened");

    let sink: &dyn AnalyticsPort = &NoopAnalyticsSink;
    sink.capture(event);
}
