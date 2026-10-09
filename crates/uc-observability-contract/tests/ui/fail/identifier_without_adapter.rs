use uc_observability_contract::uc_warn;

fn main() {
    let entry = String::from("entry");
    uc_warn!(entry_id = entry, "identifier must go through log_id()");
}
