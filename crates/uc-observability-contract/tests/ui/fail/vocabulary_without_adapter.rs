use uc_observability_contract::uc_warn;

fn main() {
    let table = String::from("clipboard_entry");
    uc_warn!(table = table, "runtime string in a vocabulary field");
}
