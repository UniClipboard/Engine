use std::error::Error;

use uc_observability_contract::error_source::io_error_kind;
use uc_observability_contract::log_fields::id;
use uc_observability_contract::{uc_debug, uc_error, uc_info, uc_trace, uc_warn};

fn main() {
    let entry = String::from("entry");
    let io = std::io::Error::from(std::io::ErrorKind::NotFound);
    uc_trace!("message only");
    uc_debug!(error_kind = "fixed", "literal field");
    uc_info!(entry_id = id(&entry), "identifier through the adapter");
    uc_warn!(
        target: "uc_test::target",
        error_kind = "fixed",
        io_error_kind = io_error_kind(&io),
        error = &io as &dyn Error,
        "every accepted shape"
    );
    uc_error!(error_kind = "fixed", entry_id = id(&entry), "last");
}
