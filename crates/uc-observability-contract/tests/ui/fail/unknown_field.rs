use uc_observability_contract::uc_warn;

fn main() {
    uc_warn!(not_in_catalog = "value", "unknown field name");
}
