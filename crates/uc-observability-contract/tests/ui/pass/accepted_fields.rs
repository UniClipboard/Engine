use std::error::Error;

use uc_observability_contract::error_source::io_error_kind;
use uc_observability_contract::log_fields::{log_id, log_vocab, log_vocab_debug};
use uc_observability_contract::{uc_debug, uc_error, uc_info, uc_trace, uc_warn};

#[derive(Debug)]
enum Phase {
    Ready,
}

fn main() {
    let entry = String::from("entry");
    let table = String::from("clipboard_entry");
    let count = 3u64;
    let io = std::io::Error::from(std::io::ErrorKind::NotFound);
    uc_trace!("message only");
    uc_debug!(error_kind = "fixed", "literal field");
    uc_info!(entry_id = log_id(&entry), "identifier through the adapter");
    uc_info!(
        table = log_vocab(&table),
        phase = log_vocab_debug(&Phase::Ready),
        reason = "literal vocabulary",
        "reviewed vocabulary through adapters"
    );
    uc_info!(
        attempt = 2u32,
        removed = true,
        elapsed_ms = Some(5u64),
        total = &count,
        "scalars, options and references"
    );
    uc_warn!(
        target: "uc_test::target",
        error_kind = "fixed",
        io_error_kind = io_error_kind(&io),
        error = &io as &dyn Error,
        "every accepted shape",
    );
    uc_error!(error_kind = "fixed", entry_id = log_id(&entry), "last");
}
