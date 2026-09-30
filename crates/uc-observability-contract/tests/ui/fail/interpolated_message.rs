use uc_observability_contract::uc_warn;

fn main() {
    let value = 1;
    uc_warn!(error_kind = "fixed", "value {}", value);
}
