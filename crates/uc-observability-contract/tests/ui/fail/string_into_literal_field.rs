use uc_observability_contract::uc_warn;

fn main() {
    let dynamic = String::from("runtime data");
    uc_warn!(error_kind = dynamic, "runtime string in a fixed-vocabulary field");
}
