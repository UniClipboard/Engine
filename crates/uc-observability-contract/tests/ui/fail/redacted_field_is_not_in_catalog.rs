use uc_observability_contract::uc_warn;

fn main() {
    uc_warn!(peer = "device", "peer identifiers are never logged");
}
