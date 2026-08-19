#[test]
fn capability_authoring_errors_are_teaching_diagnostics() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/capability/*.rs");
}
