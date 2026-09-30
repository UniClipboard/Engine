//! `uc_*!` 宏的编译期约束：合规写法通过，未登记字段、错误的值类别与内插消息在编译期失败。

#[test]
fn log_event_macros_enforce_field_catalog_at_compile_time() {
    let cases = trybuild::TestCases::new();
    cases.pass("tests/ui/pass/*.rs");
    cases.compile_fail("tests/ui/fail/*.rs");
}
