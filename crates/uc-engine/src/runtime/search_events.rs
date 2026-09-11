use uc_application::deps::SearchStatusEventPort;
use uc_application::facade::SearchStatusView;

use crate::engine::event_stream::EventSender;
use crate::operations::history::search::search_status_summary;

/// 只转发搜索负责人给出的产品状态，不读取存储或参与重建步骤。
pub(super) struct EngineSearchStatusEvents {
    pub events: EventSender,
}

impl SearchStatusEventPort for EngineSearchStatusEvents {
    fn changed(&self, status: SearchStatusView) {
        self.events.send(crate::EngineEvent::SearchStatusChanged(
            search_status_summary(status),
        ));
    }
}
